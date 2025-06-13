
use std::{
    collections::HashMap,
    ops::{Mul, Range},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use eore_api::{consts::BUS_COUNT, state::Proof};
use rand::Rng;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::InstructionError,
    signature::Signature,
    signer::Signer,
    transaction::{Transaction, TransactionError},
};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::{RpcSimulateTransactionConfig, RpcTransactionConfig},
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};
use tokio::{sync::{mpsc::UnboundedSender, Mutex, RwLock}, time::Instant};
use tracing::info;
use uuid::Uuid;
use base64::{prelude::BASE64_STANDARD, Engine};

use crate::{
    app_database::AppDatabase, bitz_utils::{
        get_auth_ix, get_cutoff, get_mine_with_global_boost_ix, get_proof, get_proof_and_config_with_busses, get_reset_ix, MineEventWithGlobalBoosts, BITZ_TOKEN_DECIMALS
    }, models::{InsertChallenge, InsertTxn, UpdateReward}, Config, EpochHashes, MessageInternalAllClients, MessageInternalMineSuccess, SubmissionWindow, WalletExtension
};

pub async fn pool_submission_system(
    app_proof: Arc<Mutex<Proof>>,
    app_epoch_hashes: Arc<RwLock<EpochHashes>>,
    app_wallet: Arc<WalletExtension>,
    app_nonce: Arc<Mutex<u64>>,
    app_prio_fee: Arc<u64>,
    rpc_client: Arc<RpcClient>,
    config: Arc<Config>,
    app_database: Arc<AppDatabase>,
    app_all_clients_sender: UnboundedSender<MessageInternalAllClients>,
    mine_success_sender: UnboundedSender<MessageInternalMineSuccess>,
    app_submission_window: Arc<RwLock<SubmissionWindow>>,
    app_client_nonce_ranges: Arc<RwLock<HashMap<Uuid, Vec<Range<u64>>>>>,
    app_last_challenge: Arc<Mutex<[u8; 32]>>,
) {
    loop {
        let lock = app_proof.lock().await;
        let old_proof = lock.clone();
        drop(lock);

        let cutoff = get_cutoff(old_proof, 1);
        if cutoff <= 0 {
            // process solutions
            let reader = app_epoch_hashes.read().await;
            let solution = reader.best_hash.solution.clone();
            drop(reader);
            if solution.is_some() {
                // Close submission window
                info!(target: "server_log", "Submission window closed.");
                let mut writer = app_submission_window.write().await;
                writer.closed = true;
                drop(writer);

                let signer = app_wallet.clone().miner_wallet.clone();

                let bus = rand::thread_rng().gen_range(0..BUS_COUNT);

                let mut success = false;
                let reader = app_epoch_hashes.read().await;
                let best_solution = reader.best_hash.solution.clone();
                let submissions = reader.submissions.clone();
                drop(reader);

                let app_app_wallet = app_wallet.clone();

                for i in 0..10 {
                    let app_wallet = app_app_wallet.clone();
                    if let Some(best_solution) = best_solution {
                        let difficulty = best_solution.to_hash().difficulty();

                        info!(target: "server_log",
                            "Starting mine submission attempt {} with difficulty {}.",
                            i, difficulty
                        );
                        info!(target: "server_log", "Submission Challenge: {}", BASE64_STANDARD.encode(old_proof.challenge));
                        let mut loaded_config = None;
                        info!(target: "server_log", "Getting latest config and busses data.");

                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        if let (Ok(p), Ok(config), Ok(_busses)) =
                            get_proof_and_config_with_busses(&rpc_client, signer.pubkey()).await
                        {
                            loaded_config = Some(config);

                            info!(target: "server_log", "Latest Challenge: {}", BASE64_STANDARD.encode(p.challenge));

                            if !best_solution.is_valid(&p.challenge) {
                                tracing::error!(target: "server_log", "SOLUTION IS NOT VALID ANYMORE!");
                                info!(target: "server_log", "Updating to latest proof.");
                                let mut lock = app_proof.lock().await;
                                *lock = p;
                                drop(lock);
                                break;
                            }
                        }

                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("Time went backwards")
                            .as_secs();
                        let mut ixs = vec![];
                        let mut prio_fee = *app_prio_fee;

                        let _ = app_all_clients_sender.send(MessageInternalAllClients {
                            text: String::from("Server is sending mine transaction..."),
                        });

                        let mut cu_limit = 565_000;
                        let should_add_reset_ix = if let Some(config) = loaded_config {
                            let time_until_reset = (config.last_reset_at + 300) - now as i64;
                            if time_until_reset <= 5 {
                                cu_limit = 625_000;
                                prio_fee += 10;
                                info!(target: "server_log", "Including reset tx.");
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };

                        info!(target: "server_log", "using priority fee of {}", prio_fee);

                        let cu_limit_ix =
                            ComputeBudgetInstruction::set_compute_unit_limit(cu_limit);
                        ixs.push(cu_limit_ix);

                        let prio_fee_ix =
                            ComputeBudgetInstruction::set_compute_unit_price(0);
                        ixs.push(prio_fee_ix);

                        let noop_ix = get_auth_ix(signer.pubkey());
                        ixs.push(noop_ix);

                        if should_add_reset_ix {
                            let reset_ix = get_reset_ix(signer.pubkey());
                            ixs.push(reset_ix);
                        }

                        let ix_mine: steel::Instruction = get_mine_with_global_boost_ix(signer.pubkey(), best_solution, bus);
                        ixs.push(ix_mine);

                        info!(target: "server_log", "built ixs getting balances...");

                        // let mut balance_before_tx = 0;

                        // if let Ok(balance) = get_ore_balance(
                        //     app_wallet.clone().miner_wallet.pubkey(),
                        //     &rpc_client.clone(),
                        // )
                        // .await
                        // {
                        //     balance_before_tx = balance
                        // }

                        // info!(target: "server_log", "got balance {} BITZ, sending to rpc_client", balance_before_tx);

                        if let Ok((hash, _slot)) = rpc_client
                            .get_latest_blockhash_with_commitment(rpc_client.commitment())
                            .await
                        {
                            info!(target: "server_log", "Got block building tx...");

                            let mut tx = Transaction::new_with_payer(&ixs, Some(&signer.pubkey()));

                            let expired_timer = Instant::now();
                            tx.sign(&[&signer], hash);

                            info!(target: "server_log", "Sending signed tx...");
                            info!(target: "server_log", "attempt: {}", i + 1);

                            let send_client = rpc_client.clone();
                            let rpc_sim_config = RpcSimulateTransactionConfig {
                                sig_verify: false,
                                ..RpcSimulateTransactionConfig::default()
                            };
                            
                            let sim_tx = tx.clone();

                            if let Ok(result) = rpc_client
                                .simulate_transaction_with_config(&sim_tx, rpc_sim_config)
                                .await
                            {
                                if let Some(tx_error) = result.value.err {
                                    if tx_error
                                        == TransactionError::InstructionError(
                                            4,
                                            InstructionError::Custom(1),
                                        )
                                        || tx_error
                                            == TransactionError::InstructionError(
                                                5,
                                                InstructionError::Custom(1),
                                            )
                                    {
                                        tracing::error!(target: "server_log", "Custom program error: Invalid Hash");
                                        break;
                                    }
                                }
                            }

                            let mut rpc_send_attempts = 1;
                            let signature = loop {
                                match send_client
                                    .send_and_confirm_transaction(&tx)
                                    .await
                                {
                                    Ok(sig) => {
                                        break Ok(sig);
                                    }
                                    Err(e) => {
                                        tracing::error!(target: "server_log", "Failed to send mine tx error: {:?}", e);
                                        tracing::error!(target: "server_log", "Attempt {} Failed to send mine transaction. retrying in 1 seconds...", rpc_send_attempts);
                                        rpc_send_attempts += 1;

                                        if rpc_send_attempts >= 5 {
                                            break Err("Failed to send tx");
                                        }
                                        tokio::time::sleep(Duration::from_millis(1500)).await;
                                    }
                                }
                            };

                            let signature = if signature.is_err() {
                                break;
                            } else {
                                signature.unwrap()
                            };

                            let (tx_message_sender, tx_message_receiver) =
                                tokio::sync::oneshot::channel::<u8>();
                            let app_app_nonce = app_nonce.clone();
                            let app_app_database = app_database.clone();
                            let app_app_config = config.clone();
                            let app_app_rpc_client = rpc_client.clone();
                            let app_app_proof = app_proof.clone();
                            let app_app_wallet = app_wallet.clone();
                            let app_app_epoch_hashes = app_epoch_hashes.clone();
                            let app_app_submission_window = app_submission_window.clone();
                            let app_app_client_nonce_ranges = app_client_nonce_ranges.clone();
                            let app_app_last_challenge = app_last_challenge.clone();
                            tokio::spawn(async move {
                                let mut stop_reciever = tx_message_receiver;
                                let app_nonce = app_app_nonce;
                                let app_database = app_app_database;
                                let app_config = app_app_config;
                                let app_rpc_client = app_app_rpc_client;
                                let app_proof = app_app_proof;
                                let app_wallet = app_app_wallet;
                                let app_epoch_hashes = app_app_epoch_hashes;
                                let app_submission_window = app_app_submission_window;
                                let app_client_nonce_ranges = app_app_client_nonce_ranges;
                                let app_last_challenge = app_app_last_challenge;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                loop {
                                    if let Ok(_) = stop_reciever.try_recv() {
                                        // Transaction has succeeded or expired
                                        break;
                                    } else {
                                        // Wait 100ms then check for updated proof
                                        tokio::time::sleep(Duration::from_millis(100)).await;

                                        info!(target: "server_log", "Checking for proof hash update.");
                                        let lock = app_proof.lock().await;
                                        let latest_proof = lock.clone();
                                        drop(lock);

                                        if old_proof.challenge.eq(&latest_proof.challenge) {
                                            // info!(target: "server_log", "Proof challenge not updated yet..");
                                            if let Ok(p) = get_proof(
                                                &app_rpc_client,
                                                app_wallet.clone().miner_wallet.pubkey(),
                                            )
                                            .await
                                            {
                                                info!(target: "server_log", "OLD PROOF CHALLENGE: {}", BASE64_STANDARD.encode(old_proof.challenge));
                                                info!(target: "server_log", "RPC PROOF CHALLENGE: {}", BASE64_STANDARD.encode(p.challenge));
                                                if old_proof.challenge.ne(&p.challenge) {
                                                    info!(target: "server_log", "Found new proof from rpc call, not websocket...");
                                                    let mut lock = app_proof.lock().await;
                                                    *lock = p;
                                                    drop(lock);

                                                    let mut lock = app_last_challenge.lock().await;
                                                    *lock = old_proof.challenge;
                                                    drop(lock);

                                                    // Add new db challenge, reset epoch_hashes,
                                                    // and open the submission window

                                                    // reset nonce
                                                    {
                                                        let mut nonce = app_nonce.lock().await;
                                                        *nonce = 0;
                                                    }
                                                    // reset client nonce ranges
                                                    {
                                                        let mut writer =
                                                            app_client_nonce_ranges.write().await;
                                                        *writer = HashMap::new();
                                                        drop(writer);
                                                    }
                                                    // reset epoch hashes
                                                    {
                                                        info!(target: "server_log", "reset epoch hashes");
                                                        let mut mut_epoch_hashes =
                                                            app_epoch_hashes.write().await;
                                                        mut_epoch_hashes.challenge = p.challenge;
                                                        mut_epoch_hashes.best_hash.solution = None;
                                                        mut_epoch_hashes.best_hash.difficulty = 0;
                                                        mut_epoch_hashes.submissions =
                                                            HashMap::new();
                                                    }
                                                    // Open submission window
                                                    info!(target: "server_log", "openning submission window.");
                                                    let mut writer =
                                                        app_submission_window.write().await;
                                                    writer.closed = false;
                                                    drop(writer);

                                                    info!(target: "server_log", "Adding new challenge to db");
                                                    let new_challenge = InsertChallenge {
                                                        pool_id: app_config.pool_id,
                                                        challenge: p.challenge.to_vec(),
                                                        rewards_earned: None,
                                                    };

                                                    while let Err(_) = app_database
                                                        .add_new_challenge(new_challenge.clone())
                                                        .await
                                                    {
                                                        tracing::error!(target: "server_log", "Failed to add new challenge to db.");
                                                        info!(target: "server_log", "Verifying challenge does not already exist.");
                                                        if let Ok(_) = app_database
                                                            .get_challenge_by_challenge(
                                                                new_challenge.challenge.clone(),
                                                            )
                                                            .await
                                                        {
                                                            info!(target: "server_log", "Challenge already exists, continuing");
                                                            break;
                                                        }

                                                        tokio::time::sleep(Duration::from_millis(
                                                            1000,
                                                        ))
                                                        .await;
                                                    }
                                                    info!(target: "server_log", "New challenge successfully added to db");

                                                    break;
                                                }
                                            }
                                        } else {
                                            let mut lock = app_last_challenge.lock().await;
                                            *lock = old_proof.challenge;
                                            drop(lock);
                                            info!(target: "server_log", "Adding new challenge to db");
                                            let new_challenge = InsertChallenge {
                                                pool_id: app_config.pool_id,
                                                challenge: latest_proof.challenge.to_vec(),
                                                rewards_earned: None,
                                            };

                                            while let Err(_) = app_database
                                                .add_new_challenge(new_challenge.clone())
                                                .await
                                            {
                                                tracing::error!(target: "server_log", "Failed to add new challenge to db.");
                                                info!(target: "server_log", "Verifying challenge does not already exist.");
                                                if let Ok(_) = app_database
                                                    .get_challenge_by_challenge(
                                                        new_challenge.challenge.clone(),
                                                    )
                                                    .await
                                                {
                                                    info!(target: "server_log", "Challenge already exists, continuing");
                                                    break;
                                                }

                                                tokio::time::sleep(Duration::from_millis(1000))
                                                    .await;
                                            }
                                            info!(target: "server_log", "New challenge successfully added to db");

                                            // reset nonce
                                            {
                                                let mut nonce = app_nonce.lock().await;
                                                *nonce = 0;
                                            }
                                            // reset client nonce ranges
                                            {
                                                let mut writer =
                                                    app_client_nonce_ranges.write().await;
                                                *writer = HashMap::new();
                                                drop(writer);
                                            }
                                            // reset epoch hashes
                                            {
                                                info!(target: "server_log", "reset epoch hashes");
                                                let mut mut_epoch_hashes =
                                                    app_epoch_hashes.write().await;
                                                mut_epoch_hashes.challenge = latest_proof.challenge;
                                                mut_epoch_hashes.best_hash.solution = None;
                                                mut_epoch_hashes.best_hash.difficulty = 0;
                                                mut_epoch_hashes.submissions = HashMap::new();
                                            }
                                            // Open submission window
                                            info!(target: "server_log", "openning submission window.");
                                            let mut writer = app_submission_window.write().await;
                                            writer.closed = false;
                                            drop(writer);

                                            break;
                                        }
                                    }
                                    tokio::time::sleep(Duration::from_millis(1000)).await;
                                }
                                return;
                            });

                            info!(target: "server_log", "SIG: {}", signature.to_string());

                            let result: Result<Signature, String> = loop {
                                if expired_timer.elapsed().as_secs() >= 200 {
                                    break Err("Transaction Expired".to_string());
                                }
                                let results = rpc_client.get_signature_statuses(&[signature]).await;
                                if let Ok(response) = results {
                                    let statuses = response.value;
                                    if let Some(status) = &statuses[0] {
                                        info!(target: "server_log", "Status: {:?}", status);
                                        if status.confirmation_status()
                                            == TransactionConfirmationStatus::Finalized
                                        {
                                            if status.err.is_some() {
                                                let e_str =
                                                    format!("Transaction Failed: {:?}", status.err);
                                                break Err(e_str);
                                            }
                                            break Ok(signature);
                                        }
                                    }
                                }

                                tokio::time::sleep(Duration::from_millis(500)).await;
                            };

                            // stop the tx sender
                            let _ = tx_message_sender.send(0);

                            match result {
                                Ok(sig) => {
                                    success = true;
                                    info!(target: "server_log", "Success!!");
                                    info!(target: "server_log", "Sig: {}", sig);
                                    let itxn = InsertTxn {
                                        txn_type: "mine".to_string(),
                                        signature: sig.to_string(),
                                        priority_fee: prio_fee as u32,
                                    };
                                    let app_db = app_database.clone();
                                    tokio::spawn(async move {
                                        while let Err(_) = app_db.add_new_txn(itxn.clone()).await {
                                            tracing::error!(target: "server_log", "Failed to add tx to db! Retrying...");
                                            tokio::time::sleep(Duration::from_millis(2000)).await;
                                        }
                                    });

                                    let app_rpc_client = rpc_client.clone();
                                    let app_app_database = app_database.clone();
                                    let app_mine_success_sender = Arc::new(mine_success_sender.clone());
                                    let app_app_proof = app_proof.clone();
                                    let app_app_config = config.clone();
                                    // let app_app_wallet = app_wallet.clone();

                                    tokio::spawn(async move {
                                        let rpc_client = app_rpc_client;
                                        let app_database = app_app_database;
                                        let mine_success_sender = app_mine_success_sender;
                                        let app_proof = app_app_proof;
                                        let app_config = app_app_config;

                                        loop {
                                            if let Ok(txn_result) = rpc_client
                                                .get_transaction_with_config(
                                                    &sig,
                                                    RpcTransactionConfig {
                                                        encoding: Some(
                                                            UiTransactionEncoding::Base64,
                                                        ),
                                                        commitment: Some(rpc_client.commitment()),
                                                        max_supported_transaction_version: None,
                                                    },
                                                )
                                                .await
                                            {
                                                let data = txn_result
                                                    .transaction
                                                    .meta
                                                    .unwrap()
                                                    .log_messages;

                                                match data {
                                                    solana_transaction_status::option_serializer::OptionSerializer::Some(data) => {
                                                        let prefix = format!("Program return: {} ", eore_api::ID.to_string());
                                                        let mut mine_event_str = "";
                                                    
                                                        for log_message in data.iter().rev() {
                                                            println!("log_message {}", log_message);
                                                            if log_message.starts_with(&prefix) {
                                                                mine_event_str = log_message.trim_start_matches(&prefix);
                                                                break;
                                                            }
                                                        }
                                                        if mine_event_str.is_empty() {
                                                            tracing::error!(target: "server_log", "tx sig result missing return data");
                                                        }

                                                        // Parse return data 
                                                        let bytes = BASE64_STANDARD.decode(mine_event_str).unwrap();

                                                        if let Ok(mine_event) = bytemuck::try_from_bytes::<MineEventWithGlobalBoosts>(&bytes) {
                                                            info!(target: "server_log", "MineEvent Global Boosts: {:?}", mine_event);
                                                            //info!(target: "submission_log", "MineEvent: {:?}", mine_event);
                                                            let encoded_challenge = BASE64_STANDARD.encode(old_proof.challenge);
                                                            info!(target: "server_log", "For Challenge: {:?}", encoded_challenge);
                                                            //info!(target: "submission_log", "For Challenge: {:?}", BASE64_STANDARD.encode(old_proof.challenge));
                                                            let full_rewards = mine_event.net_base_reward.checked_add(mine_event.net_miner_boost_reward).unwrap();
                                                            let commissions = full_rewards.mul(5).saturating_div(100);

                                                            // handle sending mine success message
                                                            let mut total_real_hashpower: u64 = 0;
                                                            let mut total_hashpower: u64 = 0;
                                                            for submission in submissions.iter() {
                                                                total_hashpower += submission.1.hashpower;
                                                                total_real_hashpower += submission.1.real_hashpower;
                                                            }
                                                            let challenge;
                                                            loop {
                                                                if let Ok(c) = app_database
                                                                    .get_challenge_by_challenge(
                                                                        old_proof.challenge.to_vec(),
                                                                    )
                                                                    .await
                                                                {
                                                                    challenge = c;
                                                                    break;
                                                                } else {
                                                                    tracing::error!(target: "server_log", 
                                                                        "Failed to get challenge by challenge! Inserting if necessary..."
                                                                    );
                                                                    let new_challenge = InsertChallenge {
                                                                        pool_id: app_config.pool_id,
                                                                        challenge: old_proof.challenge.to_vec(),
                                                                        rewards_earned: None,
                                                                    };
                                                                    while let Err(_) = app_database
                                                                        .add_new_challenge(new_challenge.clone())
                                                                        .await
                                                                    {
                                                                        tracing::error!(target: "server_log", "Failed to add new challenge to db.");
                                                                        info!(target: "server_log", "Verifying challenge does not already exist.");
                                                                        if let Ok(_) = app_database.get_challenge_by_challenge(new_challenge.challenge.clone()).await {
                                                                            info!(target: "server_log", "Challenge already exists, continuing");
                                                                            break;
                                                                        }

                                                                        tokio::time::sleep(Duration::from_millis(1000))
                                                                            .await;
                                                                    }
                                                                    info!(target: "server_log", "New challenge successfully added to db");
                                                                    tokio::time::sleep(Duration::from_millis(1000)).await;
                                                                }
                                                            }

                                                            let new_commission_rewards = vec![UpdateReward {
                                                                miner_id: app_config.commissions_miner_id,
                                                                balance: commissions,
                                                            }];

                                                            tracing::info!(target: "server_log", "Updating commissions rewards...");
                                                            while let Err(_) = app_database.update_rewards(new_commission_rewards.clone()).await {
                                                                tracing::error!(target: "server_log", "Failed to update commission rewards in db. Retrying...");
                                                                tokio::time::sleep(Duration::from_millis(500)).await;
                                                            }
                                                            tracing::info!(target: "server_log", "Updated commissions rewards");
                                                            tokio::time::sleep(Duration::from_millis(200)).await;

                                                            tokio::time::sleep(Duration::from_millis(1000)).await;
                                                            let latest_proof = { app_proof.lock().await.clone() };
                                                            let balance = (latest_proof.balance as f64)
                                                                / 10f64.powf(BITZ_TOKEN_DECIMALS as f64);

                                                            let multiplier = 0.0f64;

                                                            info!(target: "server_log", "Sending internal mine success for challenge: {}", BASE64_STANDARD.encode(old_proof.challenge));
                                                            let _ = mine_success_sender.send(
                                                                MessageInternalMineSuccess {
                                                                    difficulty,
                                                                    total_balance: balance,
                                                                    rewards: full_rewards,
                                                                    commissions,
                                                                    challenge_id: challenge.id,
                                                                    challenge: old_proof.challenge,
                                                                    best_nonce: u64::from_le_bytes(best_solution.n),
                                                                    total_hashpower,
                                                                    total_real_hashpower,
                                                                    bitz_config: loaded_config,
                                                                    multiplier,
                                                                    submissions,
                                                                    global_boosts_active: true,
                                                                },
                                                            );
                                                            tokio::time::sleep(Duration::from_millis(200)).await;
                                                        } else {
                                                            tracing::error!(target: "server_log", "Failed get MineEvent data from transaction... wtf...");
                                                            break;
                                                        }
                                                    },
                                                    solana_transaction_status::option_serializer::OptionSerializer::None => {
                                                        tracing::error!(target: "server_log", "RPC gave no transaction metadata....");
                                                        tokio::time::sleep(Duration::from_millis(2000)).await;
                                                        continue;
                                                    },
                                                    solana_transaction_status::option_serializer::OptionSerializer::Skip => {
                                                        tracing::error!(target: "server_log", "RPC gave transaction metadata should skip...");
                                                        tokio::time::sleep(Duration::from_millis(2000)).await;
                                                        continue;
                                                    },
                                                }
                                                break;
                                            } else {
                                                tracing::error!(target: "server_log", "Failed to get confirmed transaction... Come on rpc...");
                                                tokio::time::sleep(Duration::from_millis(2000))
                                                    .await;
                                            }
                                        }
                                    });

                                    loop {
                                        info!(target: "server_log", "Checking for proof hash update.");
                                        let lock = app_proof.lock().await;
                                        let latest_proof = lock.clone();
                                        drop(lock);

                                        if old_proof.challenge.eq(&latest_proof.challenge) {
                                            info!(target: "server_log", "Proof challenge not updated yet..");
                                            if let Ok(p) = get_proof(
                                                &rpc_client,
                                                app_wallet.miner_wallet.pubkey(),
                                            )
                                            .await
                                            {
                                                info!(target: "server_log", "OLD PROOF CHALLENGE: {}", BASE64_STANDARD.encode(old_proof.challenge));
                                                info!(target: "server_log", "RPC PROOF CHALLENGE: {}", BASE64_STANDARD.encode(p.challenge));

                                                if old_proof.challenge.ne(&p.challenge) {
                                                    info!(target: "server_log", "Found new proof after finalized from rpc call, not websocket...");
                                                    let mut lock = app_proof.lock().await;
                                                    *lock = p;
                                                    drop(lock);

                                                    {
                                                        let mut nonce = app_nonce.lock().await;
                                                        *nonce = 0;
                                                    }
                                                    // reset client nonce ranges
                                                    {
                                                        let mut writer =
                                                            app_client_nonce_ranges.write().await;
                                                        *writer = HashMap::new();
                                                        drop(writer);
                                                    }
                                                    // reset epoch hashes
                                                    {
                                                        info!(target: "server_log", "reset epoch hashes");
                                                        let mut mut_epoch_hashes =
                                                            app_epoch_hashes.write().await;
                                                        mut_epoch_hashes.challenge = p.challenge;
                                                        mut_epoch_hashes.best_hash.solution = None;
                                                        mut_epoch_hashes.best_hash.difficulty = 0;
                                                        mut_epoch_hashes.submissions =
                                                            HashMap::new();
                                                    }

                                                    info!(target: "server_log", "openning submission window.");
                                                    let mut writer =
                                                        app_submission_window.write().await;
                                                    writer.closed = false;
                                                    drop(writer);

                                                    info!(target: "server_log", "Adding new challenge to db");
                                                    let new_challenge = InsertChallenge {
                                                        pool_id: config.pool_id,
                                                        challenge: latest_proof.challenge.to_vec(),
                                                        rewards_earned: None,
                                                    };

                                                    while let Err(_) = app_database
                                                        .add_new_challenge(new_challenge.clone())
                                                        .await
                                                    {
                                                        tracing::error!(target: "server_log", "Failed to add new challenge to db.");
                                                        info!(target: "server_log", "Verifying challenge does not already exist.");
                                                        if let Ok(_) = app_database
                                                            .get_challenge_by_challenge(
                                                                new_challenge.challenge.clone(),
                                                            )
                                                            .await
                                                        {
                                                            info!(target: "server_log", "Challenge already exists, continuing");
                                                            break;
                                                        }

                                                        tokio::time::sleep(Duration::from_millis(
                                                            1000,
                                                        ))
                                                        .await;
                                                    }
                                                    info!(target: "server_log", "New challenge successfully added to db");

                                                    break;
                                                }
                                            }
                                        } else {
                                            let reader = app_epoch_hashes.read().await;
                                            let epoch_hashes_challenge = reader.challenge;
                                            drop(reader);

                                            if latest_proof.challenge.eq(&epoch_hashes_challenge) {
                                                // epoch_hashes challenge was already updated
                                                info!(target: "server_log", "Epoch hashes challenge already up to date!");
                                                break;
                                            } else {
                                                info!(target: "server_log", "Epoch hashes challenge was not updated yet. Updating...");
                                                {
                                                    let mut nonce = app_nonce.lock().await;
                                                    *nonce = 0;
                                                }
                                                // reset client nonce ranges
                                                {
                                                    let mut writer =
                                                        app_client_nonce_ranges.write().await;
                                                    *writer = HashMap::new();
                                                    drop(writer);
                                                }
                                                // reset epoch hashes
                                                {
                                                    info!(target: "server_log", "reset epoch hashes");
                                                    let mut mut_epoch_hashes =
                                                        app_epoch_hashes.write().await;
                                                    mut_epoch_hashes.challenge =
                                                        latest_proof.challenge;
                                                    mut_epoch_hashes.best_hash.solution = None;
                                                    mut_epoch_hashes.best_hash.difficulty = 0;
                                                    mut_epoch_hashes.submissions = HashMap::new();
                                                }
                                                // Open submission window
                                                info!(target: "server_log", "openning submission window.");
                                                let mut writer =
                                                    app_submission_window.write().await;
                                                writer.closed = false;
                                                drop(writer);
                                                info!(target: "server_log", "Adding new challenge to db");
                                                let new_challenge = InsertChallenge {
                                                    pool_id: config.pool_id,
                                                    challenge: latest_proof.challenge.to_vec(),
                                                    rewards_earned: None,
                                                };

                                                while let Err(_) = app_database
                                                    .add_new_challenge(new_challenge.clone())
                                                    .await
                                                {
                                                    tracing::error!(target: "server_log", "Failed to add new challenge to db.");
                                                    info!(target: "server_log", "Verifying challenge does not already exist.");
                                                    if let Ok(_) = app_database
                                                        .get_challenge_by_challenge(
                                                            new_challenge.challenge.clone(),
                                                        )
                                                        .await
                                                    {
                                                        info!(target: "server_log", "Challenge already exists, continuing");
                                                        break;
                                                    }

                                                    tokio::time::sleep(Duration::from_millis(1000))
                                                        .await;
                                                }
                                                info!(target: "server_log", "New challenge successfully added to db");
                                                break;
                                            }
                                        }
                                    }
                                    break;
                                }
                                Err(e) => {
                                    tracing::error!(target: "server_log", "Failed to send and confirm txn");
                                    tracing::error!(target: "server_log", "Error: {:?}", e);
                                    println!("Error: {:?}", e);
                                    // info!(target: "server_log", "increasing prio fees");
                                    // {
                                    //     let mut prio_fee = app_prio_fee.lock().await;
                                    //     if *prio_fee < 1_000_000 {
                                    //         *prio_fee += 15_000;
                                    //     }
                                    // }
                                    tokio::time::sleep(Duration::from_millis(2_000)).await;
                                }
                            }
                        } else {
                            tracing::error!(target: "server_log", "Failed to get latest blockhash. retrying...");
                            tokio::time::sleep(Duration::from_millis(1_000)).await;
                        }
                    } else {
                        tracing::error!(target: "server_log", "Solution is_some but got none on best hash re-check?");
                        tokio::time::sleep(Duration::from_millis(1_000)).await;
                    }
                }

                if !success {
                    info!(target: "server_log", "Failed to send tx. Discarding and refreshing data.");
                    // reset nonce
                    {
                        let mut nonce = app_nonce.lock().await;
                        *nonce = 0;
                    }
                    // reset client nonce ranges
                    {
                        let mut writer = app_client_nonce_ranges.write().await;
                        *writer = HashMap::new();
                        drop(writer);
                    }
                    // reset epoch hashes
                    {
                        info!(target: "server_log", "reset epoch hashes");
                        let mut mut_epoch_hashes = app_epoch_hashes.write().await;
                        mut_epoch_hashes.best_hash.solution = None;
                        mut_epoch_hashes.best_hash.difficulty = 0;
                        mut_epoch_hashes.submissions = HashMap::new();
                    }
                    // Open submission window
                    info!(target: "server_log", "openning submission window.");
                    let mut writer = app_submission_window.write().await;
                    writer.closed = false;
                    drop(writer);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            } else {
                tracing::error!(target: "server_log", "No best solution yet.");
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(1000)).await;
        };
    }
}
