use anyhow::Context;
use prometheus::{core::GenericGauge, opts, register_int_gauge};

use super::tpu_connection_manager::TpuConnectionManager;
use crate::quic_connection_utils::QuicConnectionParameters;
use crate::tpu_utils::quic_proxy_connection_manager::QuicProxyConnectionManager;
use crate::tpu_utils::tpu_connection_path::TpuConnectionPath;
use crate::tpu_utils::tpu_service::ConnectionManager::{DirectTpu, QuicProxy};

use solana_lite_rpc_core::network_utils::log_gso_workaround;
use solana_lite_rpc_core::stores::data_cache::DataCache;
use solana_lite_rpc_core::structures::transaction_sent_info::SentTransactionInfo;
use solana_lite_rpc_core::traits::leaders_fetcher_interface::LeaderFetcherInterface;
use solana_lite_rpc_core::types::SlotStream;
use solana_lite_rpc_core::AnyhowJoinHandle;
use solana_sdk::{quic::QUIC_PORT_OFFSET, signature::Keypair, slot_history::Slot};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use solana_tls_utils::new_dummy_x509_certificate;

lazy_static::lazy_static! {
    static ref NB_CLUSTER_NODES: GenericGauge<prometheus::core::AtomicI64> =
    register_int_gauge!(opts!("literpc_nb_cluster_nodes", "Number of cluster nodes in saved")).unwrap();

    static ref NB_OF_LEADERS_IN_SCHEDULE: GenericGauge<prometheus::core::AtomicI64> =
    register_int_gauge!(opts!("literpc_cached_leader", "Number of leaders in schedule cache")).unwrap();

    static ref ESTIMATED_SLOT: GenericGauge<prometheus::core::AtomicI64> =
    register_int_gauge!(opts!("literpc_estimated_slot", "Estimated slot seen by last rpc")).unwrap();
}

#[derive(Clone, Copy)]
pub struct TpuServiceConfig {
    pub fanout_slots: u64,
    pub maximum_transaction_in_queue: usize,
    pub quic_connection_params: QuicConnectionParameters,
    pub tpu_connection_path: TpuConnectionPath,
}

#[derive(Clone)]
pub struct TpuService {
    broadcast_sender: Arc<tokio::sync::broadcast::Sender<SentTransactionInfo>>,
    connection_manager: ConnectionManager,
    leader_schedule: Arc<dyn LeaderFetcherInterface>,
    config: TpuServiceConfig,
    data_cache: DataCache,
}

#[derive(Clone)]
enum ConnectionManager {
    DirectTpu {
        tpu_connection_manager: Arc<TpuConnectionManager>,
    },
    QuicProxy {
        quic_proxy_connection_manager: Arc<QuicProxyConnectionManager>,
    },
}

impl TpuService {
    pub async fn new(
        config: TpuServiceConfig,
        identity: Arc<Keypair>,
        leader_schedule: Arc<dyn LeaderFetcherInterface>,
        data_cache: DataCache,
    ) -> anyhow::Result<Self> {
        let (sender, _) = tokio::sync::broadcast::channel(config.maximum_transaction_in_queue);
        let (certificate, key) = new_dummy_x509_certificate(identity.as_ref());
        log_gso_workaround();

        let connection_manager = match config.tpu_connection_path {
            TpuConnectionPath::QuicDirectPath => {
                let tpu_connection_manager =
                    TpuConnectionManager::new(certificate, key, config.fanout_slots as usize).await;
                DirectTpu {
                    tpu_connection_manager: Arc::new(tpu_connection_manager),
                }
            }
            TpuConnectionPath::QuicForwardProxyPath {
                forward_proxy_address,
            } => {
                let quic_proxy_connection_manager =
                    QuicProxyConnectionManager::new(certificate, key, forward_proxy_address).await;

                QuicProxy {
                    quic_proxy_connection_manager: Arc::new(quic_proxy_connection_manager),
                }
            }
        };

        Ok(Self {
            leader_schedule,
            broadcast_sender: Arc::new(sender),
            connection_manager,
            config,
            data_cache,
        })
    }

    pub fn send_transaction(&self, transaction: &SentTransactionInfo) -> anyhow::Result<()> {
        self.broadcast_sender.send(transaction.clone())?;
        Ok(())
    }

    // update/reconfigure connections on slot change
    async fn update_quic_connections(
        &self,
        current_slot: Slot,
        estimated_slot: Slot,
    ) -> anyhow::Result<()> {
        let fanout = self.config.fanout_slots;
        let last_slot = estimated_slot + fanout;
        let current_slot = current_slot.saturating_sub(4);

        let cluster_nodes = self.data_cache.cluster_info.cluster_nodes.clone();

        let next_leaders = self
            .leader_schedule
            .get_slot_leaders(current_slot, last_slot)
            .await?;
        // get next leader with its tpu port
        let connections_to_keep: HashMap<_, _> = next_leaders
            .iter()
            .map(|x| {
                let contact_info = cluster_nodes.get(&x.pubkey);
                let tpu_addr = match contact_info {
                    Some(info) => info.tpu,
                    _ => None,
                };
                (x.pubkey, tpu_addr)
            })
            .filter_map(|(pubkey, maybe_addr)| {
                let tpu_addr = maybe_addr?;
                Some((pubkey, tpu_addr))
            })
            .flat_map(|(pubkey, addr)| {
                // fwd_diff: {1: 5174, 8: 3, 3: 5, 10: 3, 7: 3, 9: 4, 16: 405, 6: 6, 5: 1, 4: 8, 0: 89, 2: 3}
                let tpu_addr = SocketAddr::new(addr.ip(), addr.port() + QUIC_PORT_OFFSET);
                // let tpu_fwd1_addr = SocketAddr::new(addr.ip(), addr.port() + QUIC_PORT_OFFSET + 1);
                // let tpu_fwd16_addr = SocketAddr::new(addr.ip(), addr.port() + QUIC_PORT_OFFSET + 16);
                // vec![(pubkey, tpu_addr), (pubkey, tpu_fwd1_addr), (pubkey, tpu_fwd16_addr)]
                vec![(pubkey, tpu_addr)]
            })
            .collect();

        match &self.connection_manager {
            DirectTpu {
                tpu_connection_manager,
            } => {
                tpu_connection_manager
                    .update_connections(
                        self.broadcast_sender.clone(),
                        connections_to_keep,
                        self.data_cache.identity_stakes.get_stakes().await,
                        self.data_cache.clone(),
                        self.config.quic_connection_params,
                    )
                    .await;
            }
            QuicProxy {
                quic_proxy_connection_manager,
            } => {
                let transaction_receiver = self.broadcast_sender.subscribe();
                quic_proxy_connection_manager
                    .update_connection(
                        transaction_receiver,
                        connections_to_keep,
                        self.config.quic_connection_params,
                    )
                    .await;
            }
        }
        Ok(())
    }

    pub fn start(&self, slot_notifications: SlotStream) -> AnyhowJoinHandle {
        let this = self.clone();
        tokio::spawn(async move {
            let mut slot_notifications = slot_notifications;
            loop {
                let notification = slot_notifications
                    .recv()
                    .await
                    .context("Tpu service cannot get slot notification")?;
                this.update_quic_connections(
                    notification.processed_slot,
                    notification.estimated_processed_slot,
                )
                .await?;
            }
        })
    }
}
