use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    ops::Range,
    sync::Arc,
};

use eore_api::state::Proof;
use uuid::Uuid;

use axum::extract::ws::Message;
use futures::SinkExt;
use tokio::{
    sync::{mpsc::UnboundedReceiver, Mutex, RwLock},
    time::Instant,
};

use crate::{
    AppState, ClientMessage, EpochHashes, InternalMessageSubmission, LastPong, SubmissionWindow,
    MAX_CALCULATED_HASHPOWER, MIN_DIFF, MIN_HASHPOWER,
};

pub async fn client_message_handler_system(
    mut receiver_channel: UnboundedReceiver<ClientMessage>,
    ready_clients: Arc<Mutex<HashSet<SocketAddr>>>,
    proof: Arc<Mutex<Proof>>,
    epoch_hashes: Arc<RwLock<EpochHashes>>,
    client_nonce_ranges: Arc<RwLock<HashMap<Uuid, Vec<Range<u64>>>>>,
    app_state: Arc<RwLock<AppState>>,
    app_pongs: Arc<RwLock<LastPong>>,
    app_submission_window: Arc<RwLock<SubmissionWindow>>,
) {
    while let Some(client_message) = receiver_channel.recv().await {
        match client_message {
            ClientMessage::Pong(addr) => {
                let mut writer = app_pongs.write().await;
                writer.pongs.insert(addr, Instant::now());
                drop(writer);
            }
            ClientMessage::Ready(addr) => {
                let ready_clients = ready_clients.clone();
                tokio::spawn(async move {
                    let mut ready_clients = ready_clients.lock().await;
                    ready_clients.insert(addr);
                });
            }
            ClientMessage::Mining(addr) => {
                tracing::info!(target: "server_log", "Client {} has started mining!", addr.to_string());
            }
            ClientMessage::BestSolution(addr, solution, uuid) => {
                let app_epoch_hashes = epoch_hashes.clone();
                let app_proof = proof.clone();
                let app_client_nonce_ranges = client_nonce_ranges.clone();
                let app_state = app_state.clone();
                let app_submission_window = app_submission_window.clone();
                tokio::spawn(async move {
                    let epoch_hashes = app_epoch_hashes;
                    let proof = app_proof;
                    let client_nonce_ranges = app_client_nonce_ranges;

                    let reader = app_submission_window.read().await;
                    let submission_windows_closed = reader.closed;
                    drop(reader);

                    if submission_windows_closed {
                        // tracing::error!(target: "server_log", "{} submitted after submission window was closed!", uuid);

                        let reader = app_state.read().await;
                        if let Some(app_client_socket) = reader.sockets.get(&addr) {
                            let msg = format!("Late submission. Please make sure your hash time is under 60 seconds.");
                            let _ = app_client_socket
                                .socket
                                .lock()
                                .await
                                .send(Message::Text(msg.into()))
                                .await;
                        } else {
                            tracing::error!(target: "server_log", "Failed to get client socket for addr: {}", addr);
                            return;
                        }
                        drop(reader);
                        return;
                    }

                    let reader = client_nonce_ranges.read().await;
                    let nonce_ranges: Vec<Range<u64>> = {
                        if let Some(nr) = reader.get(&uuid) {
                            nr.clone()
                        } else {
                            tracing::error!(target: "server_log", "Client nonce range not set!");
                            return;
                        }
                    };
                    drop(reader);

                    let nonce = u64::from_le_bytes(solution.n);

                    let mut in_range = false;

                    for nonce_range in nonce_ranges.iter() {
                        if nonce_range.contains(&nonce) {
                            in_range = true;
                            break;
                        }
                    }

                    if !in_range {
                        tracing::error!(target: "server_log", "Client submitted nonce out of assigned range");
                        return;
                    }

                    let reader = app_state.read().await;
                    let miner_id;
                    if let Some(app_client_socket) = reader.sockets.get(&addr) {
                        miner_id = app_client_socket.miner_id;
                    } else {
                        tracing::error!(target: "server_log", "Failed to get client socket for addr: {}", addr);
                        return;
                    }
                    drop(reader);

                    let lock = proof.lock().await;
                    let challenge = lock.challenge;
                    drop(lock);
                    if solution.is_valid(&challenge) {
                        let diff = solution.to_hash().difficulty();
                        let submission_uuid = Uuid::new_v4();
                        // tracing::info!(target: "submission_log", "{} - {} found diff: {}", submission_uuid, pubkey_str, diff);
                        if diff >= MIN_DIFF {
                            // calculate rewards
                            let real_hashpower = MIN_HASHPOWER * 2u64.pow(diff - MIN_DIFF);
                            let mut hashpower = real_hashpower.clone();
                            if hashpower > MAX_CALCULATED_HASHPOWER {
                                hashpower = MAX_CALCULATED_HASHPOWER;
                            }
                            {
                                let reader = epoch_hashes.read().await;
                                let subs = reader.submissions.clone();
                                drop(reader);

                                if let Some(old_sub) = subs.get(&uuid) {
                                    if diff > old_sub.supplied_diff {
                                        let mut epoch_hashes = epoch_hashes.write().await;
                                        epoch_hashes.submissions.insert(
                                            uuid,
                                            InternalMessageSubmission {
                                                miner_id,
                                                supplied_nonce: nonce,
                                                supplied_diff: diff,
                                                hashpower,
                                                real_hashpower,
                                            },
                                        );
                                        if diff > epoch_hashes.best_hash.difficulty {
                                            // tracing::info!(target: "server_log", "{} - New best diff: {}", submission_uuid, diff);
                                            tracing::info!(target: "submission_log", "{} - New best diff: {}", submission_uuid, diff);
                                            epoch_hashes.best_hash.difficulty = diff;
                                            epoch_hashes.best_hash.solution = Some(solution);
                                        }
                                        drop(epoch_hashes);
                                    }
                                } else {
                                    tracing::info!(target: "submission_log", "{} - Adding {} submission diff: {} to epoch_hashes submissions.", submission_uuid, uuid, diff);
                                    let mut epoch_hashes = epoch_hashes.write().await;
                                    epoch_hashes.submissions.insert(
                                        uuid,
                                        InternalMessageSubmission {
                                            miner_id,
                                            supplied_nonce: nonce,
                                            supplied_diff: diff,
                                            hashpower,
                                            real_hashpower,
                                        },
                                    );
                                    if diff > epoch_hashes.best_hash.difficulty {
                                        // tracing::info!(target: "server_log", "{} - New best diff: {}", submission_uuid, diff);
                                        tracing::info!(target: "submission_log", "{} - New best diff: {}", submission_uuid, diff);
                                        epoch_hashes.best_hash.difficulty = diff;
                                        epoch_hashes.best_hash.solution = Some(solution);
                                    }
                                    drop(epoch_hashes);
                                    tracing::info!(target: "submission_log", "{} - Added {} submission diff: {} to epoch_hashes submissions.", submission_uuid, uuid, diff);
                                }
                            }
                        } else {
                            tracing::error!(target: "submission_log", "Diff to low, skipping submission from {}",uuid);
                            // tracing::error!(target: "server_log", "{} - {} found diff: {}", submission_uuid, pubkey_str, diff);
                        }
                    } else {
                        tracing::error!(target: "server_log", "{} returned an invalid solution!", uuid);

                        let reader = app_state.read().await;
                        if let Some(app_client_socket) = reader.sockets.get(&addr) {
                            let _ = app_client_socket.socket.lock().await.send(Message::Text("Invalid solution. If this keeps happening, please contact support.".to_string().into())).await;
                        } else {
                            tracing::error!(target: "server_log", "Failed to get client socket for addr: {}", addr);
                            return;
                        }
                        drop(reader);
                    }
                });
            }
        }
    }
}
