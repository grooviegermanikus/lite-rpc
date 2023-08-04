use std::time::{Duration, Instant};

use anyhow::bail;
use chrono::Utc;
use log::{info, trace, warn};

use prometheus::{
    core::GenericGauge, histogram_opts, opts, register_histogram, register_int_counter,
    register_int_gauge, Histogram, IntCounter,
};
use solana_sdk::slot_history::Slot;
use tokio::{sync::mpsc::Receiver, task::JoinHandle};

use crate::tpu_utils::tpu_service::TpuService;
use solana_lite_rpc_core::{
    notifications::{NotificationMsg, NotificationSender, TransactionNotification},
    tx_store::{TxProps, TxStore},
};

lazy_static::lazy_static! {
    static ref TXS_SENT: IntCounter =
        register_int_counter!("literpc_txs_sent", "Number of transactions forwarded to tpu").unwrap();
    static ref TXS_SENT_ERRORS: IntCounter =
    register_int_counter!("literpc_txs_sent_errors", "Number of errors while transactions forwarded to tpu").unwrap();
    static ref TX_BATCH_SIZES: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tx_batch_size", "batchsize of tx sent by literpc")).unwrap();
    static ref TT_SENT_TIMER: Histogram = register_histogram!(histogram_opts!(
        "literpc_txs_send_timer",
        "Time to send transaction batch",
    ))
    .unwrap();
    static ref TX_TIMED_OUT: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tx_timeout", "Number of transactions that timeout")).unwrap();
    pub static ref TXS_IN_CHANNEL: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_txs_in_channel", "Transactions in channel")).unwrap();

}

pub type WireTransaction = Vec<u8>;

#[derive(Clone, Debug)]
pub struct TransactionInfo {
    pub signature: String,
    pub slot: Slot,
    pub transaction: WireTransaction,
    pub last_valid_block_height: u64,
}
// making 250 as sleep time will effectively make lite rpc send
// (1000/250) * 5 * 512 = 10240 tps
const INTERVAL_PER_BATCH_IN_MS: u64 = 50;
const MAX_BATCH_SIZE_IN_PER_INTERVAL: usize = 2000;

/// Retry transactions to a maximum of `u16` times, keep a track of confirmed transactions
#[derive(Clone)]
pub struct TxSender {
    /// Tx(s) forwarded to tpu
    txs_sent_store: TxStore,
    /// TpuClient to call the tpu port
    tpu_service: TpuService,
}

impl TxSender {
    pub fn new(txs_sent_store: TxStore, tpu_service: TpuService) -> Self {
        Self {
            tpu_service,
            txs_sent_store,
        }
    }

    /// retry enqued_tx(s)
    async fn forward_txs(
        &self,
        transaction_infos: Vec<TransactionInfo>,
        notifier: Option<NotificationSender>,
    ) {
        if transaction_infos.is_empty() {
            return;
        }

        let histo_timer = TT_SENT_TIMER.start_timer();
        let start = Instant::now();

        let tpu_client = self.tpu_service.clone();
        let txs_sent = self.txs_sent_store.clone();

        for transaction_info in &transaction_infos {
            trace!("sending transaction {}", transaction_info.signature);
            txs_sent.insert(
                transaction_info.signature.clone(),
                TxProps {
                    status: None,
                    last_valid_blockheight: transaction_info.last_valid_block_height,
                },
            );
        }

        let forwarded_slot = tpu_client.get_estimated_slot();
        let forwarded_local_time = Utc::now();

        let mut quic_responses = vec![];
        for transaction_info in transaction_infos.iter() {
            txs_sent.insert(
                transaction_info.signature.clone(),
                TxProps::new(transaction_info.last_valid_block_height),
            );
            let quic_response = match tpu_client.send_transaction(
                transaction_info.signature.clone(),
                transaction_info.transaction.clone(),
            ).await {
                Ok(_) => {
                    TXS_SENT.inc_by(1);
                    1
                }
                Err(err) => {
                    TXS_SENT_ERRORS.inc_by(1);
                    warn!("{err}");
                    0
                }
            };
            quic_responses.push(quic_response);
        }
        if let Some(notifier) = &notifier {
            let notification_msgs = transaction_infos
                .iter()
                .enumerate()
                .map(|(index, transaction_info)| TransactionNotification {
                    signature: transaction_info.signature.clone(),
                    recent_slot: transaction_info.slot,
                    forwarded_slot,
                    forwarded_local_time,
                    processed_slot: None,
                    cu_consumed: None,
                    cu_requested: None,
                    quic_response: quic_responses[index],
                })
                .collect();
            // ignore error on sent because the channel may be already closed
            let _ = notifier.send(NotificationMsg::TxNotificationMsg(notification_msgs));
        }
        histo_timer.observe_duration();
        trace!(
            "It took {} ms to send a batch of {} transaction(s)",
            start.elapsed().as_millis(),
            transaction_infos.len()
        );
    }

    /// retry and confirm transactions every 2ms (avg time to confirm tx)
    pub fn execute(
        self,
        mut recv: Receiver<TransactionInfo>,
        notifier: Option<NotificationSender>,
    ) -> JoinHandle<anyhow::Result<()>> {
        tokio::spawn(async move {
            loop {
                let mut transaction_infos = Vec::with_capacity(MAX_BATCH_SIZE_IN_PER_INTERVAL);
                let mut timeout_interval = INTERVAL_PER_BATCH_IN_MS;

                // In solana there in sig verify stage rate is limited to 2000 txs in 50ms
                // taking this as reference
                while transaction_infos.len() <= MAX_BATCH_SIZE_IN_PER_INTERVAL {
                    let instance = tokio::time::Instant::now();
                    match tokio::time::timeout(Duration::from_millis(timeout_interval), recv.recv())
                        .await
                    {
                        Ok(value) => match value {
                            Some(transaction_info) => {
                                TXS_IN_CHANNEL.dec();

                                // duplicate transaction
                                if self
                                    .txs_sent_store
                                    .contains_key(&transaction_info.signature)
                                {
                                    continue;
                                }
                                transaction_infos.push(transaction_info);
                                // update the timeout inteval
                                timeout_interval = timeout_interval
                                    .saturating_sub(instance.elapsed().as_millis() as u64)
                                    .max(1);
                            }
                            None => {
                                log::error!("Channel Disconnected");
                                bail!("Channel Disconnected");
                            }
                        },
                        Err(_) => {
                            break;
                        }
                    }
                }

                if transaction_infos.is_empty() {
                    continue;
                }

                TX_BATCH_SIZES.set(transaction_infos.len() as i64);

                self.forward_txs(transaction_infos, notifier.clone()).await;
            }
        })
    }

    pub fn cleanup(&self, current_finalized_blochash: u64) {
        let length_before = self.txs_sent_store.len();
        self.txs_sent_store.retain(|_k, v| {
            let retain = v.last_valid_blockheight >= current_finalized_blochash;
            if !retain && v.status.is_none() {
                TX_TIMED_OUT.inc();
            }
            retain
        });
        info!(
            "Cleaned {} transactions",
            length_before - self.txs_sent_store.len()
        );
    }
}
