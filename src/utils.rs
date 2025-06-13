use std::{
    net::SocketAddr,
    ops::ControlFlow,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use axum::{
    body::Bytes, extract::{
        ws::{Message, WebSocket},
        ConnectInfo, Query, State, WebSocketUpgrade
    }, http::{Response, StatusCode},
    response::IntoResponse, 
    routing::{get, post},
    Extension, Json, Router
};
use axum_extra::{headers::authorization::Basic, TypedHeader};
use drillx::Solution;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use spl_associated_token_account::get_associated_token_address;
use tokio::sync::{mpsc::UnboundedSender, Mutex, RwLock};
use solana_sdk::{pubkey::Pubkey, signature::{Signature, Signer}};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use tracing::{info, error};

use crate::{app_database::{AppDatabase, AppDatabaseError}, app_rr_database::AppRRDatabase, bitz_utils::get_bitz_mint, models::{self, LastClaim}, AppClientConnection, AppState, ClaimsQueue, ClaimsQueueItem, ClientMessage, ClientVersion, Config, WalletExtension};

pub fn create_router() -> Router<Arc<RwLock<AppState>>> {
    Router::new()
        .route("/v2/ws", get(ws_handler_v2))
        .route("/v2/ws-pubkey", get(ws_handler_pubkey))
        //.route("/pause", post(post_pause))
        // .route( "/latest-blockhash", get(get_latest_blockhash))
        // .route("/pool/authority/pubkey", get(get_pool_authority_pubkey))
        // .route("/pool/fee_payer/pubkey", get(get_pool_fee_payer_pubkey))
        .route("/v2/signup", post(post_signup_v2))
        // .route("/signup-fee", get(get_signup_fee))
        // .route("/sol-balance", get(get_sol_balance))
        .route("/v2/confirmation-claim", post(confirmation_claim))
        .route("/v2/claim-direct", post(claim_direct))
        .route("/v2/claim", post(post_claim_v2))
        
        //.route("/stake", post(post_stake))
        //.route("/unstake", post(post_unstake))
        .route("/high-difficulty", get(get_high_difficulty))
        .route("/hashpower", get(get_hashpower))
        .route("/active-miners", get(get_connected_miners))
        .route("/timestamp", get(get_timestamp))
        .route("/miner/earnings", get(get_miner_earnings))
        .route(
            "/miner/earnings-submissions-day",
            get(get_earnings_with_challenge_and_submission_day),
        )
        .route(
            "/miner/earnings-submissions-hours",
            get(get_earnings_with_challenge_and_submission_hours),
        )
        // .route(
        //     "/miner/earnings-submissions",
        //     get(get_miner_earnings_for_submissions),
        // )
        // .route("/miner/balance", get(get_miner_balance))
        // .route("/v2/miner/balance", get(get_miner_balance_v2))
        // .route("/stake-multiplier", get(get_stake_multiplier))
        // App RR Database routes
        // .route(
        //     "/last-challenge-submissions",
        //     get(get_last_challenge_submissions),
        // )
        .route("/miner/rewards", get(get_miner_rewards))
        // .route("/miner/submissions", get(get_miner_submissions))
        .route("/miner/last-claim", get(get_miner_last_claim))
        // .route("/challenges", get(get_challenges))
        // .route("/txns/latest-mine", get(get_latest_mine_txn))}
}

#[derive(Deserialize)]
struct WsQueryParams {
    timestamp: u64,
}
async fn ws_handler_v2(
    ws: WebSocketUpgrade,
    TypedHeader(auth_header): TypedHeader<axum_extra::headers::Authorization<Basic>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(app_state): State<Arc<RwLock<AppState>>>,
    //Extension(app_config): Extension<Arc<Config>>,
    Extension(client_channel): Extension<UnboundedSender<ClientMessage>>,
    Extension(app_database): Extension<Arc<AppDatabase>>,
    query_params: Query<WsQueryParams>,
) -> impl IntoResponse {
    let msg_timestamp = query_params.timestamp;
    // info!(target:"server_log", "New WebSocket connection from: {:?}", addr);

    let pubkey = auth_header.username();
    let signed_msg = auth_header.password();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    // Signed authentication message is only valid for 30 seconds
    if (now - query_params.timestamp) >= 30 {
        return Err((StatusCode::UNAUTHORIZED, "Timestamp too old."));
    }

    // verify client
    if let Ok(user_pubkey) = Pubkey::from_str(pubkey) {
        let db_miner = app_database
            .get_miner_by_pubkey_str(pubkey.to_string())
            .await;

        let miner;
        match db_miner {
            Ok(db_miner) => {
                miner = db_miner;
            }
            Err(AppDatabaseError::QueryFailed) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "pubkey is not authorized to mine. please sign up.",
                ));
            }
            Err(AppDatabaseError::InteractionFailed) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "pubkey is not authorized to mine. please sign up.",
                ));
            }
            Err(AppDatabaseError::FailedToGetConnectionFromPool) => {
                error!(target: "server_log", "Failed to get database pool connection.");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"));
            }
            Err(_) => {
                error!(target: "server_log", "DB Error: Catch all.");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"));
            }
        }

        if !miner.enabled {
            return Err((StatusCode::UNAUTHORIZED, "pubkey is not authorized to mine"));
        }

        if let Ok(signature) = Signature::from_str(signed_msg) {
            let ts_msg = msg_timestamp.to_le_bytes();

            if signature.verify(&user_pubkey.to_bytes(), &ts_msg) {
                // info!(target: "server_log", "Client: {addr} connected with pubkey {pubkey} on V2.");
                return Ok(ws.on_upgrade(move |socket| {
                    handle_socket(
                        socket,
                        addr,
                        user_pubkey,
                        miner.id,
                        ClientVersion::V2,
                        app_state,
                        client_channel,
                    )
                }));
            } else {
                return Err((StatusCode::UNAUTHORIZED, "Sig verification failed"));
            }
        } else {
            return Err((StatusCode::UNAUTHORIZED, "Invalid signature"));
        }
    } else {
        return Err((StatusCode::UNAUTHORIZED, "Invalid pubkey"));
    }
}

#[derive(Deserialize)]
struct WsPubkeyQueryParams {
    timestamp: u64,
    pubkey: String,
}
async fn ws_handler_pubkey(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(app_state): State<Arc<RwLock<AppState>>>,
    Extension(client_channel): Extension<UnboundedSender<ClientMessage>>,
    Extension(app_database): Extension<Arc<AppDatabase>>,
    Extension(app_wallet): Extension<Arc<WalletExtension>>,
    query_params: Query<WsPubkeyQueryParams>,
) -> impl IntoResponse {
    // info!(target:"server_log", "New WebSocket connection from: {:?}", addr);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    // Signed authentication message is only valid for 30 seconds
    if (now - query_params.timestamp) >= 30 {
        return Err((StatusCode::UNAUTHORIZED, "Timestamp too old."));
    }

    // verify client
    if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
        let db_miner = app_database
            .get_miner_by_pubkey_str(user_pubkey.to_string())
            .await;

        let miner;
        match db_miner {
            Ok(db_miner) => {
                miner = db_miner;
            }
            Err(_) => {
                error!(target: "server_log", "ws_handler_pubkey DB Error: Catch all.");
                while let Err(_) = app_database
                    .signup_user_transaction(
                        user_pubkey.to_string(),
                        app_wallet.miner_wallet.pubkey().to_string(),
                    )
                    .await
                {
                    tracing::error!(target: "server_log", "ws_handler_pubkey: Failed to signup user. Retrying...");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let db_miner = app_database
            .get_miner_by_pubkey_str(user_pubkey.to_string())
            .await;

        let miner;
        match db_miner {
            Ok(db_miner) => {
                miner = db_miner;
            }
            Err(_) => {
                error!(target: "server_log", "DB Error: Catch all.");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"));
            }
        }

        if !miner.enabled {
            return Err((StatusCode::UNAUTHORIZED, "pubkey is not authorized to mine"));
        }

        info!(target: "server_log", "Client: {addr} connected with pubkey {user_pubkey} on V2.");
        return Ok(ws.on_upgrade(move |socket| {
            handle_socket(
                socket,
                addr,
                user_pubkey,
                miner.id,
                ClientVersion::V2,
                app_state,
                client_channel,
            )
        }));
    } else {
        return Err((StatusCode::UNAUTHORIZED, "Invalid pubkey"));
    }
}

async fn handle_socket(
    mut socket: WebSocket,
    who: SocketAddr,
    who_pubkey: Pubkey,
    who_miner_id: i32,
    client_version: ClientVersion,
    rw_app_state: Arc<RwLock<AppState>>,
    client_channel: UnboundedSender<ClientMessage>,
) {
    let socket_uuid = Uuid::new_v4();
    if socket
        .send(Message::Ping(Bytes::from(vec![1, 2, 3])))
        .await
        .is_ok()
    {
        tracing::debug!("Pinged {who}...");
    } else {
        error!(target: "server_log", "could not ping {who}");

        // if we can't ping we can't do anything, return to close the connection
        return;
    }

    let (sender, mut receiver) = socket.split();
    let mut app_state = rw_app_state.write().await;
    if app_state.sockets.contains_key(&who) {
        // info!(target: "server_log", "Socket addr: {who} already has an active connection");
        return;
    } else {
        // info!(target: "server_log", "Client: {} - {} connected!",who, who_pubkey.to_string());
        let new_app_client_connection = AppClientConnection {
            uuid: socket_uuid,
            pubkey: who_pubkey,
            miner_id: who_miner_id,
            client_version,
            socket: Arc::new(Mutex::new(sender)),
        };
        app_state.sockets.insert(who, new_app_client_connection);
    }
    drop(app_state);

    let _ = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if process_message(msg, socket_uuid, who, client_channel.clone()).is_break() {
                break;
            }
        }
    })
    .await;

    let mut app_state = rw_app_state.write().await;
    app_state.sockets.remove(&who);
    drop(app_state);

    // info!(target: "server_log", "Client: {} disconnected!", who_pubkey.to_string());
}

fn process_message(
    msg: Message,
    socket_uuid: Uuid,
    who: SocketAddr,
    client_channel: UnboundedSender<ClientMessage>,
) -> ControlFlow<(), ()> {
    // info!(target: "server_log", "Received message from {who}: {msg:?}");
    match msg {
        Message::Text(_t) => {
            //println!(">>> {who} sent str: {t:?}");
        }
        Message::Binary(d) => {
            // first 8 bytes are message type
            let message_type = d[0];
            match message_type {
                0 => {
                    let msg = ClientMessage::Ready(who);
                    let _ = client_channel.send(msg);
                }
                1 => {
                    let msg = ClientMessage::Mining(who);
                    let _ = client_channel.send(msg);
                }
                2 => {
                    // parse solution from message data
                    let mut solution_bytes = [0u8; 16];
                    // extract (16 u8's) from data for hash digest
                    let mut b_index = 1;
                    for i in 0..16 {
                        solution_bytes[i] = d[i + b_index];
                    }
                    b_index += 16;

                    // extract 64 bytes (8 u8's)
                    let mut nonce = [0u8; 8];
                    for i in 0..8 {
                        nonce[i] = d[i + b_index];
                    }
                    b_index += 8;

                    let mut pubkey = [0u8; 32];
                    for i in 0..32 {
                        pubkey[i] = d[i + b_index];
                    }

                    // REMOVED MINING SIGNATURE
                    // b_index += 32;

                    //let signature_bytes = d[b_index..].to_vec();
                    //if let Ok(sig_str) = String::from_utf8(signature_bytes.clone()) {
                    //if let Ok(sig) = Signature::from_str(&sig_str) {
                    let pubkey = Pubkey::new_from_array(pubkey);

                    //let mut hash_nonce_message = [0; 24];
                    //hash_nonce_message[0..16].copy_from_slice(&solution_bytes);
                    //hash_nonce_message[16..24].copy_from_slice(&nonce);

                    //if sig.verify(&pubkey.to_bytes(), &hash_nonce_message) {
                    let solution = Solution::new(solution_bytes, nonce);

                    let msg = ClientMessage::BestSolution(who, solution, socket_uuid);
                    let _ = client_channel.send(msg);
                    //} else {
                    //    error!(target: "server_log", "Client submission sig verification failed.");
                    //}
                    //} else {
                    //    error!(target: "server_log", "Failed to parse into Signature.");
                    //}
                    //}
                    //else {
                    //    error!(target: "server_log", "Failed to parse signed message from client.");
                    //}
                }
                _ => {
                    error!(target: "server_log", ">>> {} sent an invalid message", who);
                }
            }
        }
        Message::Close(c) => {
            if let Some(cf) = c {
                info!(
                    target: "server_log",
                    ">>> {} sent close with code {} and reason `{}`",
                    who, cf.code, cf.reason
                );
            } else {
                info!(target: "server_log", ">>> {who} somehow sent close message without CloseFrame");
            }
            return ControlFlow::Break(());
        }
        Message::Pong(_v) => {
            let msg = ClientMessage::Pong(who);
            let _ = client_channel.send(msg);
        }
        Message::Ping(_v) => {
            //println!(">>> {who} sent ping with {v:?}");
        }
    }

    ControlFlow::Continue(())
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

// async fn get_latest_blockhash(
//     Extension(rpc_client): Extension<Arc<RpcClient>>,
// ) -> impl IntoResponse {
//     let latest_blockhash = rpc_client.get_latest_blockhash().await.unwrap();

//     let serialized_blockhash = bincode::serialize(&latest_blockhash).unwrap();

//     let encoded_blockhash = BASE64_STANDARD.encode(serialized_blockhash);
//     Response::builder()
//         .status(StatusCode::OK)
//         .header("Content-Type", "text/text")
//         .body(encoded_blockhash)
//         .unwrap()
// }

#[derive(Deserialize)]
struct SignupParamsV2 {
    miner: String,
}

async fn post_signup_v2(
    query_params: Query<SignupParamsV2>,
    Extension(app_database): Extension<Arc<AppDatabase>>,
    Extension(wallet): Extension<Arc<WalletExtension>>,
    _body: String,
) -> impl IntoResponse {
    if let Ok(miner_pubkey) = Pubkey::from_str(&query_params.miner) {
        let db_miner = app_database
            .get_miner_by_pubkey_str(miner_pubkey.to_string())
            .await;

        match db_miner {
            Ok(miner) => {
                if miner.enabled {
                    info!(target: "server_log", "Miner account already enabled!");
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/text")
                        .body("EXISTS".to_string())
                        .unwrap();
                }
            }
            Err(AppDatabaseError::FailedToGetConnectionFromPool) => {
                error!(target: "server_log", "Failed to get database pool connection");
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body("Failed to get db pool connection".to_string())
                    .unwrap();
            }
            Err(_) => {
                info!(target: "server_log", "No miner account exists. Signing up new user.");
            }
        }

        let res = app_database
            .signup_user_transaction(
                miner_pubkey.to_string(),
                wallet.miner_wallet.pubkey().to_string(),
            )
            .await;

        match res {
            Ok(_) => {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/text")
                    .body("SUCCESS".to_string())
                    .unwrap();
            }
            Err(_) => {
                error!(target: "server_log", "Failed to add miner to database");
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body("Failed to add user to database".to_string())
                    .unwrap();
            }
        }
    } else {
        error!(target: "server_log", "Signup with invalid miner_pubkey");
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("Invalid miner pubkey".to_string())
            .unwrap();
    }
}

#[derive(Deserialize)]
struct ClaimParamsV2 {
    timestamp: u64,
    receiver_pubkey: String,
    amount: u64,
}

#[derive(Deserialize)]
struct ClaimRequest {
    pubkey: String,
    // amount: u64,
}

async fn confirmation_claim(
    query_params: Query<ClaimRequest>,
    Extension(app_database): Extension<Arc<AppRRDatabase>>,
    Extension(rpc_client): Extension<Arc<RpcClient>>,
) -> impl IntoResponse {
    let pubkey = match Pubkey::from_str(&query_params.pubkey) {
        Ok(pk) => pk,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid pubkey".to_string())),
    };

    let miner_rewards = app_database.get_miner_rewards(pubkey.to_string()).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Failed to get rewards".to_string()))?;

    let bitz_mint = get_bitz_mint();
    let receiver_token_account = get_associated_token_address(&pubkey, &bitz_mint);

    let ata_status = rpc_client.get_token_account_balance(&receiver_token_account).await;

    let ata_exists = ata_status.is_ok();

    let result = serde_json::json!({
        "balance": miner_rewards.balance,
        "requires_ata_creation": !ata_exists,
    });

    Ok((StatusCode::OK, Json(result)))
}

async fn claim_direct(
    query_params: Query<ClaimRequest>,
    Extension(app_database): Extension<Arc<AppDatabase>>,
    Extension(rpc_client): Extension<Arc<RpcClient>>,
    Extension(claims_queue): Extension<Arc<ClaimsQueue>>,
) -> impl IntoResponse {
    let pubkey = match Pubkey::from_str(&query_params.pubkey) {
        Ok(pk) => pk,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid pubkey".to_string())),
    };

    let miner_rewards = app_database
        .get_miner_rewards(pubkey.to_string())
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Miner not found".to_string()))?;

    if miner_rewards.balance < 2_000_000_000 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Minimum claim is 0.05 BITZ".to_string(),
        ));
    }

    if let Ok(last_claim) = app_database.get_last_claim(miner_rewards.miner_id).await {
        let now = chrono::Utc::now().timestamp();
        if now - last_claim.created_at.and_utc().timestamp() <= 1800 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "Cooldown not finished".to_string(),
            ));
        }
    }

    let bitz_mint = get_bitz_mint();
    let receiver_token_account = get_associated_token_address(&pubkey, &bitz_mint);

    let is_creating_ata = rpc_client
        .get_token_account_balance(&receiver_token_account)
        .await
        .is_err();

    let mut claim_amount = miner_rewards.balance;
    if is_creating_ata {
        if claim_amount >= 2_000_000_000 {
            claim_amount -= 2_000_000_000;
        } else {
            return Err((
                StatusCode::BAD_REQUEST,
                "Not enough BITZ to cover ATA creation".to_string(),
            ));
        }
    }

    let mut writer = claims_queue.queue.write().await;
    writer.insert(
        pubkey,
        ClaimsQueueItem {
            receiver_pubkey: pubkey,
            amount: claim_amount,
        },
    );
    drop(writer);

    Ok((StatusCode::OK, "Claim submitted"))
}

async fn post_claim_v2(
    TypedHeader(auth_header): TypedHeader<axum_extra::headers::Authorization<Basic>>,
    Extension(app_database): Extension<Arc<AppDatabase>>,
    Extension(claims_queue): Extension<Arc<ClaimsQueue>>,
    Extension(rpc_client): Extension<Arc<RpcClient>>,
    query_params: Query<ClaimParamsV2>,
) -> impl IntoResponse {
    let msg_timestamp = query_params.timestamp;

    let miner_pubkey_str = auth_header.username();
    let signed_msg = auth_header.password();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    // Signed authentication message is only valid for 30 seconds
    if (now - msg_timestamp) >= 30 {
        return Err((StatusCode::UNAUTHORIZED, "Timestamp too old.".to_string()));
    }
    let receiver_pubkey = match Pubkey::from_str(&query_params.receiver_pubkey) {
        Ok(pubkey) => pubkey,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Invalid receiver_pubkey provided.".to_string(),
            ))
        }
    };

    if let Ok(miner_pubkey) = Pubkey::from_str(miner_pubkey_str) {
        if let Ok(signature) = Signature::from_str(signed_msg) {
            let amount = query_params.amount;
            let mut signed_msg = vec![];
            signed_msg.extend(msg_timestamp.to_le_bytes());
            signed_msg.extend(receiver_pubkey.to_bytes());
            signed_msg.extend(amount.to_le_bytes());

            if signature.verify(&miner_pubkey.to_bytes(), &signed_msg) {
                let reader = claims_queue.queue.read().await;
                let queue = reader.clone();
                drop(reader);

                if queue.contains_key(&miner_pubkey) {
                    return Err((StatusCode::TOO_MANY_REQUESTS, "QUEUED".to_string()));
                }

                let amount = query_params.amount;

                if amount < 2_000_000_000 {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "claim minimum is 0.05 BITZ".to_string(),
                    ));
                }

                if let Ok(miner_rewards) = app_database
                    .get_miner_rewards(miner_pubkey.to_string())
                    .await
                {
                    if amount > miner_rewards.balance {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "claim amount for BITZ exceeds miner rewards balance.".to_string(),
                        ));
                    }

                    if let Ok(last_claim) =
                        app_database.get_last_claim(miner_rewards.miner_id).await
                    {
                        let last_claim_ts = last_claim.created_at.and_utc().timestamp();
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("Time went backwards")
                            .as_secs() as i64;
                        let time_difference = now - last_claim_ts;
                        if time_difference <= 1800 {
                            return Err((
                                StatusCode::TOO_MANY_REQUESTS,
                                time_difference.to_string(),
                            ));
                        }
                    }

                    let bitz_mint = get_bitz_mint();

                    let receiver_token_account =
                        get_associated_token_address(&receiver_pubkey, &bitz_mint);

                    let mut is_creating_ata = false;

                    if let Ok(response) = rpc_client
                        .get_token_account_balance(&receiver_token_account)
                        .await
                    {
                        if let Some(_amount) = response.ui_amount {
                            info!(target: "server_log", "miner has valid token account BITZ.");
                        } else {
                            info!(target: "server_log", "will create token account for miner BITZ");
                            is_creating_ata = true;
                        }
                    } else {
                        info!(target: "server_log", "Adding create ata ix for miner claim BITZ");
                        is_creating_ata = true;
                    }

                    let mut claim_amount = amount;

                    // 0.02 BITZ
                    if is_creating_ata {
                        if claim_amount >= 2_000_000_000 {
                            claim_amount = claim_amount - 2_000_000_000
                        } else {
                            tracing::error!(target: "server_log", "miner {} has not enough BITZ to claim.", miner_pubkey);
                            return Err((
                                StatusCode::BAD_REQUEST,
                                "Not enough BITZ to cover for token account generation. Each new token account requires 0.02 BITZ".to_string(),
                            ));
                        }
                    }

                    let mut writer = claims_queue.queue.write().await;
                    writer.insert(
                        miner_pubkey,
                        ClaimsQueueItem {
                            receiver_pubkey,
                            amount,
                        },
                    );
                    drop(writer);
                    return Ok((StatusCode::OK, "SUCCESS"));
                } else {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to get miner account from database".to_string(),
                    ));
                }
            } else {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "Sig verification failed".to_string(),
                ));
            }
        } else {
            return Err((StatusCode::UNAUTHORIZED, "Invalid signature".to_string()));
        }
    } else {
        error!(target: "server_log", "Claim with invalid pubkey");
        return Err((StatusCode::BAD_REQUEST, "Invalid Pubkey".to_string()));
    }
}

#[derive(Deserialize)]
struct ConnectedMinersParams {
    pubkey: Option<String>,
}
async fn get_connected_miners(
    query_params: Query<ConnectedMinersParams>,
    State(app_state): State<Arc<RwLock<AppState>>>,
) -> impl IntoResponse {
    let reader = app_state.read().await;
    let socks = reader.sockets.clone();
    drop(reader);

    if let Some(pubkey_str) = &query_params.pubkey {
        if let Ok(user_pubkey) = Pubkey::from_str(&pubkey_str) {
            let mut connection_count = 0;

            for (_addr, client_connection) in socks.iter() {
                if user_pubkey.eq(&client_connection.pubkey) {
                    connection_count += 1;
                }
            }

            return Response::builder()
                .status(StatusCode::OK)
                .body(connection_count.to_string())
                .unwrap();
        } else {
            error!(target: "server_log", "Get connected miners with invalid pubkey");
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("Invalid Pubkey".to_string())
                .unwrap();
        }
    } else {
        return Response::builder()
            .status(StatusCode::OK)
            .body(socks.len().to_string())
            .unwrap();
    }
}

async fn get_high_difficulty(
    query_params: Query<ConnectedMinersParams>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    let pubkey = query_params.pubkey.clone().unwrap_or_default();

    let res = app_rr_database
        .get_high_difficulty(
            pubkey,
        )
        .await;

    match res {
        Ok(high_diff) => Ok(Json(high_diff)),
        Err(_) => Err("Failed to get difficulty for miner".to_string()),
    }
}

async fn get_hashpower(
    query_params: Query<ConnectedMinersParams>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    let pubkey = query_params.pubkey.clone().unwrap_or_default();

    let res = app_rr_database
        .get_hashpower(
            pubkey,
        )
        .await;

    match res {
        Ok(high_diff) => Ok(Json(high_diff)),
        Err(_) => Err("Failed to get hashpower for miner".to_string()),
    }
}

async fn get_timestamp() -> impl IntoResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();
    return Response::builder()
        .status(StatusCode::OK)
        .body(now.to_string())
        .unwrap();
}

async fn get_miner_earnings(
    query_params: Query<PubkeyAndPeriodParam>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    let one_day = Duration::from_secs(60 * 60 * 24 * 1);
    let check_end_time = query_params.end_time - one_day;

    if check_end_time > query_params.start_time {
        return Err("Maximum time period is one day".to_string());
    }
    if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
        let res = app_rr_database
            .get_earning_in_period_by_pubkey(
                user_pubkey.to_string(),
                query_params.start_time.naive_utc(),
                query_params.end_time.naive_utc(),
            )
            .await;

        match res {
            Ok(earnings) => Ok(Json(earnings)),
            Err(_) => Err("Failed to get earnings for miner".to_string()),
        }
    } else {
        return Err("Invalid public key".to_string());
    }
}

#[derive(Deserialize)]
struct PubkeyParam {
    pubkey: String,
}
#[derive(Deserialize)]
struct PubkeyAndPeriodParam {
    pubkey: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    start_time: DateTime<Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    end_time: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct FullMinerRewards {
    bitz: u64,
    bitz_decimal: f64,
}

async fn get_miner_rewards(
    query_params: Query<PubkeyParam>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
        let res = app_rr_database
            .get_miner_rewards(user_pubkey.to_string())
            .await;

        match res {
            Ok(rewards) => {
                let decimal_bal =
                    rewards.balance as f64 / 10f64.powf(eore_api::consts::TOKEN_DECIMALS as f64);
                let response = FullMinerRewards {
                    bitz: rewards.balance,
                    bitz_decimal: decimal_bal,
                };
                return Ok(Json(response));
            }
            Err(_) => {
                error!(target: "server_log", "get_miner_rewards endpoint: failed to get rewards balance from db for {}", user_pubkey.to_string());
                return Err("Failed to get balance".to_string());
            }
        }
    } else {
        return Err("Invalid public key".to_string());
    }
}

#[derive(Deserialize)]
struct GetLastClaimParams {
    pubkey: String,
}
async fn get_miner_last_claim(
    query_params: Query<GetLastClaimParams>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
    Extension(app_config): Extension<Arc<Config>>,
) -> Result<Json<LastClaim>, String> {
    if app_config.stats_enabled {
        if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
            let res = app_rr_database
                .get_last_claim_by_pubkey(user_pubkey.to_string())
                .await;

            match res {
                Ok(last_claim) => Ok(Json(last_claim)),
                Err(_) => Err("Failed to get last claim for miner".to_string()),
            }
        } else {
            Err("Invalid public key".to_string())
        }
    } else {
        return Err("Stats not enabled for this server.".to_string());
    }
}

async fn get_earnings_with_challenge_and_submission_day(
    query_params: Query<PubkeyParam>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    info!(target: "server_log", "get_miner_earnings_for_submissions_day: {:?}", query_params.pubkey);

    if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
        let res = app_rr_database
            .get_earnings_with_challenge_and_submission_day(
                user_pubkey.to_string()
            )
            .await;

        match res {
            Ok(earnings) => Ok(Json(earnings)),
            Err(_) => Err("Failed to get earnings for miner".to_string()),
        }
    } else {
        return Err("Invalid public key".to_string());
    }
}

async fn get_earnings_with_challenge_and_submission_hours(
    query_params: Query<PubkeyParam>,
    Extension(app_rr_database): Extension<Arc<AppRRDatabase>>,
) -> impl IntoResponse {
    info!(target: "server_log", "get_miner_earnings_for_submissions_hours: {:?}", query_params.pubkey);

    if let Ok(user_pubkey) = Pubkey::from_str(&query_params.pubkey) {
        let res = app_rr_database
            .get_earnings_with_challenge_and_submission_hours(
                user_pubkey.to_string()
            )
            .await;

        match res {
            Ok(earnings) => Ok(Json(earnings)),
            Err(_) => Err("Failed to get earnings for miner".to_string()),
        }
    } else {
        return Err("Invalid public key".to_string());
    }
}