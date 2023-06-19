// This class will manage the lifecycle for a transaction
// It will send, replay if necessary and confirm by listening to blocks

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    tpu_utils::tpu_service::TpuService,
    tx_sender::{TxSender, WireTransaction},
};
use anyhow::bail;
use solana_lite_rpc_core::{
    block_store::{BlockInformation, BlockStore},
    notifications::NotificationSender,
};
use solana_rpc_client::rpc_client::SerializableTransaction;
use solana_sdk::{commitment_config::CommitmentConfig, transaction::VersionedTransaction};
use tokio::{
    sync::mpsc::{self, Sender, UnboundedSender},
    task::JoinHandle,
    time::Instant,
};

#[derive(Clone)]
pub struct TransactionServiceBuilder {
    tx_sender: TxSender,
    tpu_service: TpuService,
    max_nb_txs_in_queue: usize,
}

impl TransactionServiceBuilder {
    pub fn new(
        tx_sender: TxSender,
        tpu_service: TpuService,
        max_nb_txs_in_queue: usize,
    ) -> Self {
        Self {
            tx_sender,
            tpu_service,
            max_nb_txs_in_queue,
        }
    }

    pub async fn start(
        &self,
        // notifier: Option<NotificationSender>,
        max_retries: usize,
    ) -> (TransactionService, JoinHandle<String>) {
        let (transaction_channel, tx_recv) = mpsc::channel(self.max_nb_txs_in_queue);

        let tx_sender = self.tx_sender.clone();
        let tpu_service = self.tpu_service.clone();
        let exit_signal = Arc::new(AtomicBool::new(false));
        let exit_signal_t = exit_signal.clone();
        let jh_services: JoinHandle<String> = tokio::spawn(async move {
            let tpu_service_fx = tpu_service.start(exit_signal_t.clone());

            let tx_sender_jh =
                tx_sender
                    .clone()
                    .execute(tx_recv, exit_signal_t.clone());

            tokio::select! {
                res = tpu_service_fx => {
                    format!("{res:?}")
                },
                res = tx_sender_jh => {
                    format!("{res:?}")
                },
            }
        });

        (
            TransactionService {
                transaction_channel,
                exit_signal,
                max_retries,
            },
            jh_services,
        )
    }
}

#[derive(Clone)]
pub struct TransactionService {
    pub transaction_channel: Sender<(String, WireTransaction, u64)>,
    pub exit_signal: Arc<AtomicBool>,
    pub max_retries: usize,
}

impl TransactionService {
    pub async fn send_transaction(
        &self,
        raw_tx: Vec<u8>,
        max_retries: Option<u16>,
    ) -> anyhow::Result<String> {
        let tx = match bincode::deserialize::<VersionedTransaction>(&raw_tx) {
            Ok(tx) => tx,
            Err(err) => {
                bail!(err.to_string());
            }
        };
        let signature = tx.signatures[0];

        let slot = 9999; // FIXME

        let raw_tx_clone = raw_tx.clone();
        let max_replay = max_retries.map_or(self.max_retries, |x| x as usize);
        if let Err(e) = self
            .transaction_channel
            .send((signature.to_string(), raw_tx, slot))
            .await
        {
            bail!(
                "Internal error sending transaction on send channel error {}",
                e
            );
        }

        Ok(signature.to_string())
    }

    pub fn stop(&self) {
        self.exit_signal.store(true, Ordering::Relaxed)
    }
}
