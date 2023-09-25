use std::net::SocketAddr;

use anyhow::bail;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::inbound::proxy_listener;
use crate::tls_self_signed_pair_generator::SelfSignedTlsConfigProvider;
use crate::validator_identity::ValidatorIdentity;
use log::info;
use tokio::sync::mpsc;
use solana_lite_rpc_core::structures::transaction_sent_info::SentTransactionInfo;
use crate::outbound::tx_sender::{execute, adapter_packets_to_tsi};
use crate::shared::ForwardPacket;

pub struct QuicForwardProxy {
    // endpoint: Endpoint,
    validator_identity: ValidatorIdentity,
    tls_config: Arc<SelfSignedTlsConfigProvider>,
    pub proxy_listener_addr: SocketAddr,
}

impl QuicForwardProxy {
    pub async fn new(
        proxy_listener_addr: SocketAddr,
        tls_config: Arc<SelfSignedTlsConfigProvider>,
        validator_identity: ValidatorIdentity,
    ) -> anyhow::Result<Self> {
        info!("Quic proxy uses validator identity {}", validator_identity);

        Ok(Self {
            proxy_listener_addr,
            validator_identity,
            tls_config,
        })
    }

    pub async fn start_services(self) -> anyhow::Result<()> {
        let exit_signal = Arc::new(AtomicBool::new(false));

        // let (transaction_channel, tx_recv) = mpsc::channel(self.max_nb_txs_in_queue);
        let (transaction_channel, forwarder_channel) = mpsc::channel::<ForwardPacket>(1000);
        let (forwarder_channel2, tx_recv) = mpsc::channel::<SentTransactionInfo>(1000);

        let proxy_listener =
            proxy_listener::ProxyListener::new(self.proxy_listener_addr, self.tls_config);

        let quic_proxy = tokio::spawn(async move {
            proxy_listener
                .listen(&transaction_channel)
                .await
                .expect("proxy listen service");
        });

        let validator_identity = self.validator_identity.clone();
        let exit_signal_clone = exit_signal.clone();


        // let forwarder: AnyhowJoinHandle = tokio::spawn(tx_forwarder(
        //     validator_identity,
        //     forward_receiver,
        //     exit_signal_clone,
        // ));

        // broadcast to active connections
        let (sender_active_connections, _) =
            tokio::sync::broadcast::channel::<SentTransactionInfo>(1000);
        let broadcast_sender = Arc::new(sender_active_connections);

        let adapter = adapter_packets_to_tsi(forwarder_channel2, forwarder_channel, broadcast_sender.clone());

        let tx_sender_jh = execute(tx_recv, broadcast_sender.clone());

        tokio::select! {
            res = quic_proxy => {
                bail!("TPU Quic Proxy server exited unexpectedly {res:?}");
            },
            res = adapter => {
                bail!("TPU Quic packet adapter exited unexpectedly {res:?}");
            },
            res = tx_sender_jh => {
                bail!("TPU Quic Tx forwarder exited unexpectedly {res:?}");
            },
        }
    }
}
