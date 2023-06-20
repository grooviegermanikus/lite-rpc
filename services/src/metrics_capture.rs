use std::sync::Arc;

use log::info;
use prometheus::{core::GenericGauge, opts, register_int_gauge};
use serde::{Deserialize, Serialize};
use solana_lite_rpc_core::tx_store::TxStore;
use solana_transaction_status::TransactionConfirmationStatus;
use tokio::{sync::RwLock, task::JoinHandle};

lazy_static::lazy_static! {
    static ref TXS_IN_STORE: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_txs_in_store", "Transactions in store")).unwrap();
}

#[cfg(all(tokio_unstable, not(loom)))]
#[cfg_attr(docsrs, doc(cfg(tokio_unstable)))]
lazy_static::lazy_static! {
    static ref TOKIO_TASKS: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_tasks", "Tokio tasks in lite rpc")).unwrap();
    static ref TOKIO_QUEUEDEPTH: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_blocking_queue_depth", "Tokio tasks in blocking queue in lite rpc")).unwrap();
    static ref TOKIO_INJQUEUEDEPTH: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_injection_queue_depth", "Tokio tasks in injection queue in lite rpc")).unwrap();
    static ref TOKIO_NB_BLOCKING_THREADS: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_blocking_threads", "Tokio blocking threads in lite rpc")).unwrap();
    static ref TOKIO_NB_IDLE_THREADS: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_idle_threads", "Tokio idle threads in lite rpc")).unwrap();
    static ref TOKIO_REMOTE_SCHEDULED_COUNT: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_tokio_remote_scheduled", "Tokio remote scheduled tasks")).unwrap();
    static ref STD_THREADS: GenericGauge<prometheus::core::AtomicI64> = register_int_gauge!(opts!("literpc_threads", "Nb of threads used by literpc")).unwrap();
}

/// Background worker which captures metrics
#[derive(Clone)]
pub struct MetricsCapture {
    // txs_store: TxStore,
    // metrics: Arc<RwLock<Metrics>>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Metrics {
}

impl MetricsCapture {
    // pub fn new(txs_store: TxStore) -> Self {
    pub fn new() -> Self {
        Self {
            // txs_store,
            // metrics: Default::default(),
        }
    }

    pub async fn get_metrics(&self) -> Metrics {
        todo!()
    }

    pub fn capture(self) -> JoinHandle<anyhow::Result<()>> {
        let mut one_second = tokio::time::interval(std::time::Duration::from_secs(1));

        tokio::spawn(async move {
            info!("Capturing Metrics");

            loop {
                one_second.tick().await;

                // NOOP

            }
        })
    }
}
