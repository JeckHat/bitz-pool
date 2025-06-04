use std::{sync::Arc, time::Duration};

use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction
};
use spl_associated_token_account::get_associated_token_address;
use solana_transaction_status::TransactionConfirmationStatus;
use tokio::time::Instant;
use tracing::{error, info};

use crate::{
    app_database::AppDatabase,
    bitz_utils::{get_bitz_mint, BITZ_TOKEN_DECIMALS},
    models::{InsertClaim, InsertTxn, UpdateReward},
    ClaimsQueue
};

pub async fn claim_system(
    claims_queue: Arc<ClaimsQueue>,
    rpc_client: Arc<RpcClient>,
    wallet: Arc<Keypair>,
    app_database: Arc<AppDatabase>,
) {
    loop {
        let mut claim = None;
        let reader = claims_queue.queue.read().await;
        let item = reader.iter().next();
        if let Some(item) = item {
            claim = Some((item.0.clone(), item.1.clone()));
        }
        drop(reader);

        if let Some((miner_pubkey, claim_queue_item)) = claim {
            info!(target: "server_log", "Processing claim");
            let bitz_mint = get_bitz_mint();
            let receiver_pubkey = claim_queue_item.receiver_pubkey;
            let receiver_token_account = get_associated_token_address(&receiver_pubkey, &bitz_mint);

            let prio_fee: u32 = 0;

            let mut is_creating_ata = false;
            let mut ixs = Vec::new();
            let prio_fee_ix = ComputeBudgetInstruction::set_compute_unit_price(prio_fee as u64);
            ixs.push(prio_fee_ix);

            if let Ok(response) = rpc_client
                .get_token_account_balance(&receiver_token_account)
                .await
            {
                if let Some(_amount) = response.ui_amount {
                    info!(target: "server_log", "miner has valid token account BITZ.");
                } else {
                    info!(target: "server_log", "will create token account for miner BITZ");
                    is_creating_ata = true;
                    ixs.push(
                        spl_associated_token_account::instruction::create_associated_token_account(
                            &wallet.pubkey(),
                            &receiver_pubkey,
                            &eore_api::consts::MINT_ADDRESS,
                            &spl_token::id(),
                        ),
                    )
                }
            } else {
                info!(target: "server_log", "Adding create ata ix for miner claim BITZ");
                is_creating_ata = true;
                ixs.push(
                    spl_associated_token_account::instruction::create_associated_token_account(
                        &wallet.pubkey(),
                        &receiver_pubkey,
                        &eore_api::consts::MINT_ADDRESS,
                        &spl_token::id(),
                    ),
                )
            }

            let amount = claim_queue_item.amount;

            let mut claim_amount = amount;

            // 0.02 BITZ
            if is_creating_ata {
                if claim_amount >= 2_000_000_000 {
                    claim_amount = claim_amount - 2_000_000_000
                } else {
                    error!(target: "server_log", "miner {} has not enough BITZ to claim.", miner_pubkey);
                    let mut writer = claims_queue.queue.write().await;
                    writer.remove(&miner_pubkey);
                    drop(writer);
                    continue;
                }
            }

            if claim_amount > 0 {
                let ix = crate::bitz_utils::get_claim_ix(
                    wallet.pubkey(),
                    receiver_token_account,
                    claim_amount,
                );
                ixs.push(ix);
            }

            if let Ok((hash, _slot)) = rpc_client
                .get_latest_blockhash_with_commitment(rpc_client.commitment())
                .await
            {
                let expired_timer = Instant::now();
                let mut tx = Transaction::new_with_payer(&ixs, Some(&wallet.pubkey()));

                tx.sign(&[&wallet], hash);

                let rpc_config = RpcSendTransactionConfig {
                    preflight_commitment: Some(rpc_client.commitment().commitment),
                    ..RpcSendTransactionConfig::default()
                };

                let mut attempts = 0;
                let mut signature: Option<Signature> = None;
                loop {
                    match rpc_client
                        .send_transaction_with_config(&tx, rpc_config)
                        .await
                    {
                        Ok(sig) => {
                            signature = Some(sig);
                            break;
                        }
                        Err(e) => {
                            error!(target: "server_log", "Failed to send claim transaction: {:?}. Retrying in 2 seconds...", e);
                            tokio::time::sleep(Duration::from_millis(2000)).await;
                            attempts += 1;
                            if attempts >= 5 {
                                error!(target: "server_log", "Failed to send claim transaction after 5 attempts.");
                                break;
                            }
                        }
                    }
                }

                if signature.is_some() {
                    let signature = signature.unwrap();
                    let result: Result<Signature, String> = loop {
                        if expired_timer.elapsed().as_secs() >= 120 {
                            break Err("Transaction Expired".to_string());
                        }
                        let results = rpc_client.get_signature_statuses(&[signature]).await;
                        if let Ok(response) = results {
                            let statuses = response.value;
                            if let Some(status) = &statuses[0] {
                                if status.confirmation_status()
                                    == TransactionConfirmationStatus::Finalized
                                {
                                    if status.err.is_some() {
                                        let e_str = format!("Transaction Failed: {:?}", status.err);
                                        break Err(e_str);
                                    }
                                    break Ok(signature);
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    };

                    match result {
                        Ok(sig) => {
                            let amount_dec = amount as f64 / 10f64.powf(BITZ_TOKEN_DECIMALS as f64);
                            info!(target: "server_log", "Miner {} successfully claimed {} BITZ.\nSig: {}", miner_pubkey.to_string(), amount_dec, sig.to_string());

                            // TODO: use transacions, or at least put them into one query
                            let miner = app_database
                                .get_miner_by_pubkey_str(miner_pubkey.to_string())
                                .await
                                .unwrap();
                            let db_pool = app_database
                                .get_pool_by_authority_pubkey(wallet.pubkey().to_string())
                                .await
                                .unwrap();
                            while let Err(_) = app_database
                                .decrease_miner_reward(UpdateReward {
                                    miner_id: miner.id,
                                    balance: amount,
                                })
                                .await
                            {
                                error!(target: "server_log", "Failed to decrease miner rewards! Retrying...");
                                tokio::time::sleep(Duration::from_millis(2000)).await;
                            }
                            while let Err(_) = app_database
                                .update_pool_claimed(
                                    wallet.pubkey().to_string(),
                                    amount,
                                )
                                .await
                            {
                                error!(target: "server_log", "Failed to increase pool claimed amount! Retrying...");
                                tokio::time::sleep(Duration::from_millis(2000)).await;
                            }

                            let itxn = InsertTxn {
                                txn_type: "claim".to_string(),
                                signature: sig.to_string(),
                                priority_fee: prio_fee,
                            };
                            while let Err(_) = app_database.add_new_txn(itxn.clone()).await {
                                error!(target: "server_log", "Failed to increase pool claimed amount! Retrying...");
                                tokio::time::sleep(Duration::from_millis(2000)).await;
                            }

                            let txn_id;
                            loop {
                                if let Ok(ntxn) = app_database.get_txn_by_sig(sig.to_string()).await
                                {
                                    txn_id = ntxn.id;
                                    break;
                                } else {
                                    error!(target: "server_log", "Failed to get tx by sig! Retrying...");
                                    tokio::time::sleep(Duration::from_millis(2000)).await;
                                }
                            }

                            let iclaim = InsertClaim {
                                miner_id: miner.id,
                                pool_id: db_pool.id,
                                txn_id,
                                amount,
                            };
                            while let Err(_) = app_database.add_new_claim(iclaim).await {
                                error!(target: "server_log", "Failed add new claim to db! Retrying...");
                                tokio::time::sleep(Duration::from_millis(2000)).await;
                            }

                            let mut writer = claims_queue.queue.write().await;
                            writer.remove(&miner_pubkey);
                            drop(writer);

                            info!(target: "server_log", "Claim successfully processed!");
                        }
                        Err(e) => {
                            error!(target: "server_log", "ERROR: {:?}", e);
                        }
                    }
                }
            } else {
                error!(target: "server_log", "Failed to confirm transaction, will retry on next iteration.");
                let mut writer = claims_queue.queue.write().await;
                writer.remove(&miner_pubkey);
                drop(writer);
                continue;
            }
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
