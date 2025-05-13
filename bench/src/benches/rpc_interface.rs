use anyhow::{bail, Context, Error};

use futures::future::join_all;
use futures::TryFutureExt;
use itertools::Itertools;
use log::{debug, info, trace, warn};

use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client::rpc_client::SerializableTransaction;
use solana_rpc_client_api::client_error::ErrorKind;
use solana_rpc_client_api::config::RpcSendTransactionConfig;

use solana_sdk::clock::Slot;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::TransactionConfirmationStatus;
use std::collections::{HashMap, HashSet};

use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;

pub fn create_rpc_client(rpc_url: &Url) -> RpcClient {
    RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed())
}

#[derive(Clone)]
pub enum ConfirmationResponseFromRpc {
    // RPC error on send_transaction
    SendError(Arc<ErrorKind>),
    // (sent slot confirmed slot, ..., ...)
    Success(Slot, Slot, TransactionConfirmationStatus, Duration),
    // timout waiting for confirmation status
    Timeout(Duration),
}

pub async fn send_and_confirm_bulk_transactions(
    send_rpc: &RpcClient,
    supportive_rpc: &RpcClient,
    txs: &[VersionedTransaction],
    max_timeout: Duration,
) -> anyhow::Result<Vec<(Signature, ConfirmationResponseFromRpc)>> {
    let send_config = RpcSendTransactionConfig {
        skip_preflight: true,
        preflight_commitment: None,
        encoding: None,
        max_retries: None,
        min_context_slot: None,
    };

    trace!("Polling for next slot(confirmed) ..");
    let send_slot = poll_next_slot_start(supportive_rpc)
        .await
        .context("poll for next start slot")?;
    trace!("Send slot: {}", send_slot);

    let started_at = Instant::now();
    trace!(
        "Sending {} transactions via RPC (retries=off) ..",
        txs.len()
    );
    let batch_sigs_or_fails = join_all(txs.iter().map(|tx| {
        send_rpc
            .send_transaction_with_config(tx, send_config)
            .map_err(|e| e.kind)
    }))
    .await;

    let after_send_slot = supportive_rpc
        .get_slot_with_commitment(CommitmentConfig::confirmed())
        .await
        .context("get slot afterwards")?;

    if after_send_slot - send_slot > 0 {
        warn!(
            "Slot advanced during sending transactions: {} -> {}",
            send_slot, after_send_slot
        );
    } else {
        debug!("Sucessfully sent transactions in same slot: {}", send_slot);
    }

    let num_sent_ok = batch_sigs_or_fails
        .iter()
        .filter(|sig_or_fail| sig_or_fail.is_ok())
        .count();
    let num_sent_failed = batch_sigs_or_fails
        .iter()
        .filter(|sig_or_fail| sig_or_fail.is_err())
        .count();

    for (i, tx_sig) in txs.iter().enumerate() {
        let tx_sent = batch_sigs_or_fails[i].is_ok();
        if tx_sent {
            trace!("- tx_sent {}", tx_sig.get_signature());
        } else {
            trace!("- tx_fail {}", tx_sig.get_signature());
        }
    }
    let elapsed = started_at.elapsed();
    debug!(
        "send_transaction successful for {} txs in {:.03}s",
        num_sent_ok,
        elapsed.as_secs_f32()
    );
    debug!(
        "send_transaction failed to send {} txs in {:.03}s",
        num_sent_failed,
        elapsed.as_secs_f32()
    );

    if num_sent_failed > 0 {
        warn!(
            "Some transactions failed to send: {} out of {}",
            num_sent_failed,
            txs.len()
        );
        bail!("Failed to send all transactions");
    }

    let mut pending_status_set: HashSet<Signature> = HashSet::new();
    batch_sigs_or_fails
        .iter()
        .filter(|sig_or_fail| sig_or_fail.is_ok())
        .for_each(|sig_or_fail| {
            pending_status_set.insert(sig_or_fail.as_ref().unwrap().to_owned());
        });
    let mut result_status_map: HashMap<Signature, ConfirmationResponseFromRpc> = HashMap::new();

    info!("Waiting {:?} before checking status ...", max_timeout);
    let timeout_at = started_at + max_timeout;
    while Instant::now() < timeout_at {
        info!("..");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    {
        assert_eq!(
            pending_status_set.len() + result_status_map.len(),
            num_sent_ok,
            "Items must move between pending+result"
        );

        let sigs = txs
            .iter()
            .map(|tx| tx.get_signature())
            .cloned()
            .collect_vec();

        let tx_statuses = supportive_rpc.get_signature_statuses(&sigs).await?.value;
        assert_eq!(tx_statuses.len(), sigs.len());

        for (tx_sig, tx_status) in sigs.iter().zip(tx_statuses) {
            // note that we will see tx_sigs we did not send

            trace!("sig {:?} status: {:?}", tx_sig, tx_status);
            let Some(tx_status) = tx_status else {
                // no status -> tx will remain in pending list and later marked as timedout
                continue;
            };

            let landed_slot = tx_status.slot;

            if pending_status_set.remove(tx_sig) {
                trace!(
                    "take status for sig {:?} and landed_slot: {:?}",
                    tx_sig,
                    landed_slot
                );
                let prev_value = result_status_map.insert(
                    *tx_sig,
                    ConfirmationResponseFromRpc::Success(
                        send_slot,
                        landed_slot,
                        // note: this is not optimal as we do not cover finalized here
                        TransactionConfirmationStatus::Confirmed,
                        Duration::from_secs(9999),
                    ),
                );
                assert!(prev_value.is_none(), "Must not override existing value");
            }
        } // -- END for tx status loop
    }

    let total_time_elapsed_checking = started_at.elapsed();

    // all transactions which remain in pending list are considered timed out
    for tx_sig in pending_status_set.clone() {
        pending_status_set.remove(&tx_sig);
        result_status_map.insert(
            tx_sig,
            ConfirmationResponseFromRpc::Timeout(total_time_elapsed_checking),
        );
    }

    let result_as_vec = batch_sigs_or_fails
        .into_iter()
        .enumerate()
        .map(|(i, sig_or_fail)| match sig_or_fail {
            Ok(tx_sig) => {
                let confirmation = result_status_map
                    .get(&tx_sig)
                    .expect("consistent map with all tx")
                    .clone()
                    .to_owned();
                (tx_sig, confirmation)
            }
            Err(send_error) => {
                let tx_sig = txs[i].get_signature();
                let confirmation = ConfirmationResponseFromRpc::SendError(Arc::new(send_error));
                (*tx_sig, confirmation)
            }
        })
        .collect_vec();

    Ok(result_as_vec)
}

pub async fn poll_next_slot_start(rpc_client: &RpcClient) -> Result<Slot, Error> {
    let started_at = Instant::now();
    let mut last_slot: Option<Slot> = None;
    let mut i = 1;
    // try to catch slot start
    let send_slot = loop {
        if i > 500 {
            bail!("Timeout waiting for slot change");
        }

        let iteration_ends_at = started_at + Duration::from_millis(i * 30);
        let slot = rpc_client
            .get_slot_with_commitment(CommitmentConfig::confirmed())
            .await?;
        trace!("polling slot {}", slot);
        if let Some(last_slot) = last_slot {
            if last_slot + 1 == slot {
                break slot;
            }
        }
        last_slot = Some(slot);
        tokio::time::sleep_until(iteration_ends_at).await;
        i += 1;
    };
    Ok(send_slot)
}
