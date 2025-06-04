use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::Duration
};
use axum::{
    body::Bytes, extract::{
        ws::{Message, WebSocket},
    }, http::Method, Extension
};
use clap::Parser;
use drillx::Solution;
use futures::{stream::SplitSink, SinkExt};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair},
    signer::Signer
};
use serde::{Deserialize, Serialize};
use tokio::{sync::{Mutex, RwLock}, time::Instant};
use tower_http::{cors::CorsLayer, trace::{DefaultMakeSpan, TraceLayer}};
use uuid::Uuid;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

use crate::{
    app_database::{AppDatabase, AppDatabaseError},
    app_rr_database::AppRRDatabase,
    bitz_utils::{get_proof, proof_pubkey},
    models::{ChallengeWithDifficulty, SubmissionWithPubkey},
    systems::{
        claim_system::claim_system, client_message_handler_system::client_message_handler_system, handle_ready_clients_system::handle_ready_clients_system, message_text_all_clients_system::message_text_all_clients_system, pong_tracking_system::pong_tracking_system, pool_mine_success_system::pool_mine_success_system, pool_submission_system::pool_submission_system, proof_tracking_system::proof_tracking_system
    }, utils::create_router
};

mod app_database;
mod app_rr_database;
mod bitz_utils;
mod models;
mod schema;
mod systems;
mod message;
mod utils;

const MIN_DIFF: u32 = 12;
const MIN_HASHPOWER: u64 = 80; // difficulty 12
const MAX_CALCULATED_HASHPOWER: u64 = 327_680; // difficulty 24
const DIAMOND_HANDS_DAYS: u64 = 7;
const NFT_DISTRIBUTION_DAYS: u64 = 7;

pub struct Config {
    password: String,
    pool_id: i32,
    stats_enabled: bool,
    signup_fee: f64,
    commissions_pubkey: String,
    commissions_miner_id: i32,
    commission_amount: i32,
}

pub struct EpochHashes {
    challenge: [u8; 32],
    best_hash: BestHash,
    submissions: HashMap<Uuid, InternalMessageSubmission>,
}

pub struct BestHash {
    solution: Option<Solution>,
    difficulty: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct InternalMessageSubmission {
    miner_id: i32,
    supplied_diff: u32,
    supplied_nonce: u64,
    hashpower: u64,
    real_hashpower: u64,
}

#[derive(Clone)]
struct WalletExtension {
    miner_wallet: Arc<Keypair>,
    fee_wallet: Arc<Keypair>,
}

struct AppState {
    sockets: HashMap<SocketAddr, AppClientConnection>,
    paused: bool,
}

#[derive(Clone)]
struct AppClientConnection {
    uuid: Uuid,
    pubkey: Pubkey,
    miner_id: i32,
    client_version: ClientVersion,
    socket: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Clone)]
enum ClientVersion {
    V2,
}

pub struct LastPong {
    pongs: HashMap<SocketAddr, Instant>,
}

struct ClaimsQueue {
    queue: RwLock<HashMap<Pubkey, ClaimsQueueItem>>,
}

#[derive(Clone, Copy)]
struct ClaimsQueueItem {
    receiver_pubkey: Pubkey,
    amount: u64,
}

struct SubmissionWindow {
    closed: bool,
}

#[derive(Clone)]
pub struct BoostMultiplierCache {
    item: Vec<BoostMultiplierData>,
    last_updated_at: Instant,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BoostMultiplierData {
    boost_mint: String,
    staked_balance: f64,
    total_stake_balance: f64,
    multiplier: u64,
}

#[derive(Clone)]
pub struct LastChallengeSubmissionsCache {
    item: Vec<SubmissionWithPubkey>,
    last_updated_at: Instant,
}

#[derive(Clone)]
pub struct ChallengesCache {
    item: Vec<ChallengeWithDifficulty>,
    last_updated_at: Instant,
}

#[derive(Debug)]
pub enum ClientMessage {
    Ready(SocketAddr),
    Mining(SocketAddr),
    Pong(SocketAddr),
    BestSolution(SocketAddr, Solution, Uuid),
}

pub struct MessageInternalMineSuccess {
    difficulty: u32,
    total_balance: f64,
    rewards: u64,
    commissions: u64,
    challenge_id: i32,
    challenge: [u8; 32],
    best_nonce: u64,
    total_hashpower: u64,
    total_real_hashpower: u64,
    bitz_config: Option<eore_api::state::Config>,
    multiplier: f64,
    submissions: HashMap<Uuid, InternalMessageSubmission>,
    global_boosts_active: bool,
}

pub struct MessageInternalAllClients {
    text: String,
}

#[derive(Parser, Debug)]
#[command(version, author, about, long_about = None)]
struct Args {
    #[arg(
        long,
        value_name = "priority fee",
        help = "Number of microlamports to pay as priority fee per transaction",
        default_value = "0",
        global = true
    )]
    priority_fee: u64,
    #[arg(
        long,
        value_name = "signup fee",
        help = "Amount of sol users must send to sign up for the pool",
        default_value = "0",
        global = true
    )]
    signup_fee: f64,
    #[arg(long, short, action, help = "Enable stats endpoints")]
    stats: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    let args = Args::parse();

    let server_logs = tracing_appender::rolling::daily("./logs", "bitz-pool-server.log");
    let (server_logs, _guard) = tracing_appender::non_blocking(server_logs);
    let server_log_layer = tracing_subscriber::fmt::layer()
        .with_writer(server_logs)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() == "server_log"
        }));

    let submission_logs = tracing_appender::rolling::daily("./logs", "bitz-pool-submissions.log");
    let (submission_logs, _guard) = tracing_appender::non_blocking(submission_logs);
    let submission_log_layer = tracing_subscriber::fmt::layer()
        .with_writer(submission_logs)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() == "submission_log"
        }));

    let console_log_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false) // disable ANSI color codes
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() == "server_log"
                || metadata.target() == "submission_log"
        }));

    tracing_subscriber::registry()
        .with(server_log_layer)
        .with(submission_log_layer)
        .with(console_log_layer)
        .init();

    let wallet_path_str = std::env::var("WALLET_PATH").expect("WALLET_PATH must be set.");
    let fee_wallet_path_str = std::env::var("FEE_WALLET_PATH").expect("FEE_WALLET_PATH must be set.");

    let rpc_url = std::env::var("RPC_URL").expect("RPC_URL must be set.");
    let rpc_2_url = match std::env::var("RPC_2_URL") {
        Ok(url) => {
            url
        },
        Err(_) => {
            println!("Invalid RPC_2_URL, defaulting to original RPC_URL");
            rpc_url.clone()
        }
    };
    let rpc_ws_url = std::env::var("RPC_WS_URL").expect("RPC_WS_URL must be set.");
    let password = std::env::var("PASSWORD").expect("PASSWORD must be set.");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set.");
    let database_rr_url = std::env::var("DATABASE_RR_URL").expect("DATABASE_RR_URL must be set.");
    let commission_pubkey_env = std::env::var("COMMISSION_PUBKEY").expect("COMMISSION_PUBKEY must be set.");
    let commission_pubkey = match Pubkey::from_str(&commission_pubkey_env) {
        Ok(pk) => {
            pk
        },
        Err(_) => {
            panic!("Invalid COMMISSION_PUBKEY")
        }
    };

    let commission_amount = std::env::var("COMMISSION_AMOUNT")
        .expect("COMMISSION_AMOUNT must be set.")
        .parse::<i32>()
        .expect("COMMISSION_AMOUNT must be an integer.");

    let app_database = Arc::new(AppDatabase::new(database_url));
    let app_rr_database = Arc::new(AppRRDatabase::new(database_rr_url));

    let priority_fee = Arc::new(args.priority_fee);

    let app_cache_boost_multiplier: Arc<RwLock<BoostMultiplierCache>> = Arc::new(RwLock::new(BoostMultiplierCache {
        item: vec![],
        last_updated_at: Instant::now(),
    }));

    let app_cache_last_challenge_submissions: Arc<RwLock<LastChallengeSubmissionsCache>> = Arc::new(RwLock::new(LastChallengeSubmissionsCache {
        item: vec![],
        last_updated_at: Instant::now(),
    }));

    let app_cache_challenges: Arc<RwLock<ChallengesCache>> = Arc::new(RwLock::new(ChallengesCache {
        item: vec![],
        last_updated_at: Instant::now(),
    }));

    let wallet_path = Path::new(&wallet_path_str);
    let wallet = read_keypair_file(wallet_path)
        .expect("Failed to load keypair from file: {wallet_path_str}");

    let wallet_path = Path::new(&fee_wallet_path_str);
    let fee_wallet = read_keypair_file(wallet_path)
        .expect("Failed to load keypair from file: {wallet_path_str}");

    info!(target: "server_log", "establishing rpc connection...");
    let rpc_client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
    let rpc_2_client = RpcClient::new_with_commitment(rpc_2_url, CommitmentConfig::confirmed());

    info!(target: "server_log", "Getting latest blockhash.");
    let lbhash;
    loop {
        match rpc_client.get_latest_blockhash_with_commitment(CommitmentConfig { commitment: CommitmentLevel::Finalized }).await {
                Ok(lb) => {
                    lbhash = lb;
                    break;
                },
                Err(e) => {
                    error!(target: "server_log", "Failed to get initial blockhash for cache. E: {:?}\n Retrying in 2 secs...", e);
                    tokio::time::sleep(Duration::from_secs(2000)).await;
                }
        };
    }

    info!(target: "server_log", "loading eth balance...");
    let balance = if let Ok(balance) = rpc_client.get_balance(&wallet.pubkey()).await {
        balance
    } else {
        return Err("Failed to load balance".into());
    };

    info!(target: "server_log", "Balance: {:.2}", balance as f64 / 1_000_000_000 as f64);

    if balance < 100_000 {
        return Err("Sol balance is too low!".into());
    }

    let proof = get_proof(&rpc_client, wallet.pubkey())
        .await.expect("Failed to load proof.");

    info!(target: "server_log", "Validating pool exists in db");
    let db_pool = app_database
        .get_pool_by_authority_pubkey(wallet.pubkey().to_string())
        .await;

    match db_pool {
        Ok(_) => {}
        Err(AppDatabaseError::FailedToGetConnectionFromPool) => {
            panic!("Failed to get database pool connection");
        }
        Err(_) => {
            info!(target: "server_log", "Pool missing from database. Inserting...");
            let proof_pubkey = proof_pubkey(wallet.pubkey());
            let result = app_database
                .add_new_pool(wallet.pubkey().to_string(), proof_pubkey.to_string())
                .await;

            if result.is_err() {
                panic!("Failed to create pool in database");
            }
        }
    }

    info!(target: "server_log", "Validating commissions receiver is in db");
    let commission_miner_id;
    match app_database
        .get_miner_by_pubkey_str(commission_pubkey.to_string())
        .await
    {
        Ok(miner) => {
            info!(target: "server_log", "Found commissions receiver in db.");
            commission_miner_id = miner.id;
        }
        Err(_) => {
            info!(target: "server_log", "Failed to get commissions receiver account from database.");
            info!(target: "server_log", "Inserting Commissions receiver account...");

            match app_database
                .signup_user_transaction(commission_pubkey.to_string(), wallet.pubkey().to_string())
                .await
            {
                Ok(_) => {
                    info!(target: "server_log", "Successfully inserted Commissions receiver account...");
                    if let Ok(m) = app_database
                        .get_miner_by_pubkey_str(commission_pubkey.to_string())
                        .await
                    {
                        commission_miner_id = m.id;
                    } else {
                        panic!("Failed to get commission miner id")
                    }
                }
                Err(_) => {
                    panic!("Failed to insert comissions receiver account")
                }
            }
        }
    }

    let db_pool = app_database
        .get_pool_by_authority_pubkey(wallet.pubkey().to_string())
        .await
        .unwrap();

    info!(target: "server_log", "Validating current challenge for pool exists in db");
    let result = app_database
        .get_challenge_by_challenge(proof.challenge.to_vec())
        .await;

    match result {
        Ok(_) => {}
        Err(AppDatabaseError::FailedToGetConnectionFromPool) => {
            panic!("Failed to get database pool connection");
        }
        Err(_) => {
            info!(target: "server_log", "Challenge missing from database. Inserting...");
            let new_challenge = models::InsertChallenge {
                pool_id: db_pool.id,
                challenge: proof.challenge.to_vec(),
                rewards_earned: None,
            };
            let result = app_database.add_new_challenge(new_challenge).await;

            if result.is_err() {
                panic!("Failed to create challenge in database");
            }
        }
    }

    let config = Arc::new(Config {
        password,
        pool_id: db_pool.id,
        stats_enabled: true,
        signup_fee: args.signup_fee,
        commissions_pubkey: commission_pubkey.to_string(),
        commissions_miner_id: commission_miner_id,
        commission_amount,
    });

    let epoch_hashes = Arc::new(RwLock::new(EpochHashes {
        challenge: proof.challenge,
        best_hash: BestHash {
            solution: None,
            difficulty: 0,
        },
        submissions: HashMap::new(),
    }));

    let wallet_extension = Arc::new(WalletExtension {
        miner_wallet: Arc::new(wallet),
        fee_wallet: Arc::new(fee_wallet),
    });

    let proof_ext = Arc::new(Mutex::new(proof));
    let nonce_ext = Arc::new(Mutex::new(0u64));

    let client_nonce_ranges = Arc::new(RwLock::new(HashMap::new()));

    let shared_state = Arc::new(RwLock::new(AppState {
        sockets: HashMap::new(),
        paused: false,
    }));

    let ready_clients = Arc::new(Mutex::new(HashSet::new()));

    let pongs = Arc::new(RwLock::new(LastPong {
        pongs: HashMap::new(),
    }));

    let claims_queue = Arc::new(ClaimsQueue {
        queue: RwLock::new(HashMap::new()),
    });

    let submission_window: Arc<RwLock<SubmissionWindow>> = Arc::new(RwLock::new(SubmissionWindow { closed: false }));

    let rpc_client = Arc::new(rpc_client);
    let rpc_2_client = Arc::new(rpc_2_client);

    let last_challenge = Arc::new(Mutex::new([0u8; 32]));

    let app_rpc_client = rpc_client.clone();
    let app_wallet = wallet_extension.clone();
    let app_claims_queue = claims_queue.clone();
    let app_app_database = app_database.clone();

    tokio::spawn(async move {
        claim_system(
            app_claims_queue,
            app_rpc_client,
            app_wallet.miner_wallet.clone(),
            app_app_database,
        )
        .await;
    });

    // Track client pong timings
    let app_pongs = pongs.clone();
    let app_state = shared_state.clone();
    tokio::spawn(async move {
        pong_tracking_system(app_pongs, app_state).await;
    });

    let app_wallet = wallet_extension.clone();
    let app_proof = proof_ext.clone();
    let app_last_challenge = last_challenge.clone();
    // Establish webocket connection for tracking pool proof changes.
    tokio::spawn(async move {
        proof_tracking_system(
            rpc_ws_url,
            app_wallet.miner_wallet.clone(),
            app_proof,
            app_last_challenge,
        )
        .await;
    });

    let (client_message_sender, client_message_receiver) =
        tokio::sync::mpsc::unbounded_channel::<ClientMessage>();

    // Handle client messages
    let app_ready_clients = ready_clients.clone();
    let app_proof = proof_ext.clone();
    let app_epoch_hashes = epoch_hashes.clone();
    let app_client_nonce_ranges = client_nonce_ranges.clone();
    let app_state = shared_state.clone();
    let app_pongs = pongs.clone();
    let app_submission_window = submission_window.clone();
    tokio::spawn(async move {
        client_message_handler_system(
            client_message_receiver,
            app_ready_clients,
            app_proof,
            app_epoch_hashes,
            app_client_nonce_ranges,
            app_state,
            app_pongs,
            app_submission_window,
        )
        .await;
    });

    // Handle ready clients
    let app_shared_state = shared_state.clone();
    let app_proof = proof_ext.clone();
    let app_epoch_hashes = epoch_hashes.clone();
    let app_nonce = nonce_ext.clone();
    let app_client_nonce_ranges = client_nonce_ranges.clone();
    let app_ready_clients = ready_clients.clone();
    let app_submission_window = submission_window.clone();
    tokio::spawn(async move {
        handle_ready_clients_system(
            app_shared_state,
            app_proof,
            app_epoch_hashes,
            app_ready_clients,
            app_nonce,
            app_client_nonce_ranges,
            app_submission_window,
        )
        .await;
    });

    let (mine_success_sender, mine_success_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MessageInternalMineSuccess>();

    let (all_clients_sender, all_clients_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MessageInternalAllClients>();

    let app_proof = proof_ext.clone();
    let app_epoch_hashes = epoch_hashes.clone();
    let app_wallet = wallet_extension.clone();
    let app_nonce = nonce_ext.clone();
    let app_prio_fee = priority_fee.clone();
    let app_config = config.clone();
    let app_app_database = app_database.clone();
    let app_all_clients_sender = all_clients_sender.clone();
    let app_submission_window = submission_window.clone();
    let app_client_nonce_ranges = client_nonce_ranges.clone();
    let app_last_challenge = last_challenge.clone();
    let app_rpc_client = rpc_client.clone();
    
    tokio::spawn(async move {
        pool_submission_system(
            app_proof,
            app_epoch_hashes,
            app_wallet,
            app_nonce,
            app_prio_fee,
            app_rpc_client,
            app_config,
            app_app_database,
            app_all_clients_sender,
            mine_success_sender,
            app_submission_window,
            app_client_nonce_ranges,
            app_last_challenge,
        )
        .await;
    });
    
    let app_shared_state = shared_state.clone();
    let app_app_database = app_database.clone();
    let app_config = config.clone();
    let app_wallet = wallet_extension.clone();
    tokio::spawn(async move {
        let app_database = app_app_database;
        pool_mine_success_system(
            app_shared_state,
            app_database,
            app_config,
            app_wallet,
            mine_success_receiver,
        ).await;
    });

    let app_shared_state = shared_state.clone();
    tokio::spawn(async move {
        message_text_all_clients_system(
            app_shared_state,
            all_clients_receiver
        ).await;
    });

    let cors = CorsLayer::new()
        .allow_methods([Method::GET])
        .allow_origin(tower_http::cors::Any);

    let client_channel = client_message_sender.clone();
    let app_shared_state = shared_state.clone();

    let app = create_router()
        .with_state(app_shared_state)
        .layer(Extension(app_database))
        .layer(Extension(app_rr_database))
        .layer(Extension(config))
        .layer(Extension(wallet_extension))
        .layer(Extension(client_channel))
        .layer(Extension(rpc_client))
        .layer(Extension(client_nonce_ranges))
        .layer(Extension(claims_queue))
        .layer(Extension(submission_window))
        // Logging
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3105").await.unwrap();
    info!(target: "server_log", "listening on {}", listener.local_addr().unwrap());

    let app_shared_state = shared_state.clone();
    tokio::spawn(async move {
        ping_check_system(&app_shared_state).await;
    });

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();


    Ok(())
}

async fn ping_check_system(shared_state: &Arc<RwLock<AppState>>) {
    loop {
        // send ping to all sockets
        let app_state = shared_state.read().await;
        let socks = app_state.sockets.clone();
        drop(app_state);

        let mut handles = Vec::new();
        for (who, socket) in socks.iter() {
            let who = who.clone();
            let socket = socket.clone();
            handles.push(tokio::spawn(async move {
                if socket
                    .socket
                    .lock()
                    .await
                    .send(Message::Ping(Bytes::from(vec![1, 2, 3])))
                    // .send(Message::Ping(vec![1, 2, 3]))
                    .await
                    .is_ok()
                {
                    return None;
                } else {
                    return Some((who.clone(), socket.pubkey.clone()));
                }
            }));
        }

        // remove any sockets where ping failed
        for handle in handles {
            match handle.await {
                Ok(Some((who, pubkey))) => {
                    error!(target: "server_log", "Got error sending ping to client: {} on pk: {}.", who, pubkey);
                    let mut app_state = shared_state.write().await;
                    app_state.sockets.remove(&who);
                }
                Ok(None) => {}
                Err(_) => {
                    error!(target: "server_log", "Got error sending ping to client.");
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}