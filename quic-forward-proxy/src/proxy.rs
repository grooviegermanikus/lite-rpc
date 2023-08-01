use std::net::SocketAddr;

use anyhow::bail;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;

use crate::inbound::proxy_listener;
use crate::outbound::tx_forward::tx_forwarder;
use crate::tls_self_signed_pair_generator::SelfSignedTlsConfigProvider;
use crate::util::AnyhowJoinHandle;
use crate::validator_identity::ValidatorIdentity;
use log::info;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use spl_memo::solana_program::example_mocks::solana_sdk::signature::Keypair;
use tokio::time::sleep;
use crate::shared::ForwardPacket;
use crate::test_client::sample_data_factory::{build_raw_sample_tx, build_sample_tx};

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

    pub async fn send_example_tx(&self, rpc_url: String) {
        // /Users/stefan/mango/solana-wallet/literpc-test-testnet.json
        // pubkey GjSM5dB6zbdBhvHrpRjtXSRB3gUfCShQgExsXYRQkbsA
        let keypair = solana_sdk::signature::Keypair::from_bytes(
            &[217, 56, 114, 203, 108, 170, 215, 46, 6, 28, 119,
                236, 239, 5, 66, 165, 44, 229, 75, 149, 85, 116, 249,
                195, 43, 85, 3, 167, 61, 45, 64, 79, 233, 190, 91, 95,
                2, 28, 64, 44, 74, 99, 68, 204, 211, 232, 168, 178, 92,
                231, 122, 192, 19, 57, 177, 23, 68, 144, 229, 30, 163,
                52, 67, 13]).unwrap();
        let exit_signal = Arc::new(AtomicBool::new(false));

        let (forwarder_channel, forward_receiver) = tokio::sync::mpsc::channel(100_000);

        let validator_identity = self.validator_identity.clone();
        let exit_signal_clone = exit_signal.clone();
        let forwarder: AnyhowJoinHandle = tokio::spawn(tx_forwarder(
            validator_identity,
            forward_receiver,
            exit_signal_clone,
        ));

        let rpc_client = Arc::new(RpcClient::new(rpc_url.clone()));
        let recent_blockhash = rpc_client.get_latest_blockhash().await.unwrap();

        info!("using recent blockhash {} from {}", recent_blockhash, rpc_url.clone());

        let sample_tx = build_sample_tx(&keypair, recent_blockhash);

        let forward_packet = ForwardPacket {
            transactions: vec![sample_tx],
            // testnet dallas
            tpu_address: "139.178.82.223:8003".parse().unwrap(),
            // tpu_address: "127.0.0.1:1033".parse().unwrap(),
        };
        info!("Sent sample tx to forwarder:");
        for tx in &forward_packet.transactions {
            info!("- tx {}", tx.signatures[0]);
        }
        forwarder_channel.send(forward_packet).await.unwrap();
        sleep(Duration::from_millis(50)).await;

        exit_signal.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = forwarder.await.unwrap();
        info!("Graceful shutdown of forwarder");
    }

    pub async fn start_services(self) -> anyhow::Result<()> {
        let exit_signal = Arc::new(AtomicBool::new(false));

        let (forwarder_channel, forward_receiver) = tokio::sync::mpsc::channel(100_000);

        let proxy_listener =
            proxy_listener::ProxyListener::new(self.proxy_listener_addr, self.tls_config);

        let exit_signal_clone = exit_signal.clone();
        let quic_proxy = tokio::spawn(async move {
            proxy_listener
                .listen(exit_signal_clone.clone(), forwarder_channel)
                .await
                .expect("proxy listen service");
        });

        let validator_identity = self.validator_identity.clone();
        let exit_signal_clone = exit_signal.clone();
        let forwarder: AnyhowJoinHandle = tokio::spawn(tx_forwarder(
            validator_identity,
            forward_receiver,
            exit_signal_clone,
        ));

        tokio::select! {
            res = quic_proxy => {
                bail!("TPU Quic Proxy server exited unexpectedly {res:?}");
            },
            res = forwarder => {
                bail!("TPU Quic Tx forwarder exited unexpectedly {res:?}");
            },
        }
    }
}
