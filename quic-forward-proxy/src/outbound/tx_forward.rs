use crate::quic_util::SkipServerVerification;
use crate::quinn_auto_reconnect::AutoReconnect;
use crate::shared::ForwardPacket;
use crate::util::timeout_fallback;
use crate::validator_identity::ValidatorIdentity;
use anyhow::{bail, Context};
use fan::tokio::mpsc::FanOut;
use futures::future::join_all;
use log::{debug, info, warn};
use quinn::{
    ClientConfig, Endpoint, EndpointConfig, IdleTimeout, TokioRuntime, TransportConfig, VarInt,
};
use solana_sdk::quic::QUIC_MAX_TIMEOUT_MS;
use solana_sdk::transaction::VersionedTransaction;
use solana_streamer::nonblocking::quic::ALPN_TPU_PROTOCOL_ID;
use solana_streamer::tls_certificates::new_self_signed_tls_certificate;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{channel, Receiver};
use tracing::error;

const MAX_PARALLEL_STREAMS: usize = 6;
pub const PARALLEL_TPU_CONNECTION_COUNT: usize = 4;

pub async fn tx_forwarder_count_only(
    validator_identity: ValidatorIdentity,
    mut transaction_channel: Receiver<ForwardPacket>,
    exit_signal: Arc<AtomicBool>,
) -> anyhow::Result<()> {

    let mut count = 0;

    loop {
        if exit_signal.load(Ordering::Relaxed) {
            bail!("exit signal received");
        }

        let forward_packet = transaction_channel
            .recv()
            .await
            .expect("channel closed unexpectedly");
        count += forward_packet.transactions.len();


        // must be 300000
        if count % 1000 == 0 {
            info!("received {} messages from forwarder", count);
        }

    }

}

// takes transactions from upstream clients and forwards them to the TPU
pub async fn tx_forwarder(
    validator_identity: ValidatorIdentity,
    mut transaction_channel: Receiver<ForwardPacket>,
    exit_signal: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    info!("TPU Quic forwarder started");

    let endpoint = new_endpoint_with_validator_identity(validator_identity).await;

    let mut agents: HashMap<SocketAddr, FanOut<ForwardPacket>> = HashMap::new();

    let count = Arc::new(AtomicI64::new(0));

    loop {
        if exit_signal.load(Ordering::Relaxed) {
            bail!("exit signal received");
        }

        let forward_packet = transaction_channel
            .recv()
            .await
            .expect("channel closed unexpectedly");
        let tpu_address = forward_packet.tpu_address;

        count.fetch_add(forward_packet.transactions.len() as i64, Ordering::Relaxed);
        let cnt = count.load(Ordering::Relaxed);
        if cnt % 1000 == 0 {
            info!("received {} transactions for sending to TPU", cnt);
        }

        // if cnt > 3000 {
        //     error!("BOOOOM");
        //     bail!("BOOOOM");
        // }

        // with this: received 300000 transactions for sending to TPU
        // continue; // FIXME remove

        agents.entry(tpu_address).or_insert_with(|| {
            let mut senders = Vec::new();
            for connection_idx in 1..PARALLEL_TPU_CONNECTION_COUNT {
                let (sender, mut receiver) = channel::<ForwardPacket>(1000);
                senders.push(sender);
                let exit_signal = exit_signal.clone();
                let endpoint_copy = endpoint.clone();
                let count = count.clone();
                tokio::spawn(async move {
                    debug!(
                        "Start Quic forwarder agent #{} for TPU {}",
                        connection_idx, tpu_address
                    );
                    if exit_signal.load(Ordering::Relaxed) {
                        return;
                    }

                    let auto_connection = AutoReconnect::new(endpoint_copy, tpu_address);

                    let _exit_signal_copy = exit_signal.clone();
                    while let Some(packet) = receiver.recv().await {
                        assert_eq!(packet.tpu_address, tpu_address, "routing error");

                        let mut transactions_batch = packet.transactions;

                        while let Ok(more) = receiver.try_recv() {
                            transactions_batch.extend(more.transactions);
                        }

                        debug!(
                            "forwarding transaction batch of size {} to address {}",
                            transactions_batch.len(),
                            packet.tpu_address
                        );

                        let result = timeout_fallback(send_tx_batch_to_tpu(
                            &auto_connection,
                            &transactions_batch,
                        ))
                        .await
                        .context(format!(
                            "send txs to tpu node {}",
                            auto_connection.target_address
                        ));

                        if result.is_err() {
                            warn!(
                                "got send_txs_to_tpu_static error {:?} - loop over errors",
                                result
                            );
                        } else {
                            debug!("send_txs_to_tpu_static sent {}", transactions_batch.len());
                            debug!(
                                "Outbound connection stats: {}",
                                &auto_connection.connection_stats().await
                            );
                            // count.fetch_add(transactions_batch.len() as i64, Ordering::Relaxed);
                        }



                    } // -- while all packtes from channel

                    info!(
                        "Quic forwarder agent #{} for TPU {} exited",
                        connection_idx, tpu_address
                    );
                });
            }

            FanOut::new(senders)
        }); // -- new agent

        let agent_channel = agents.get(&tpu_address).unwrap();

        timeout_fallback(agent_channel.send(forward_packet))
            .await
            .context("send to agent channel")
            .expect("must not time out")
            .expect("must receive");
    } // -- loop over transactions from upstream channels

    // not reachable
}

/// takes a validator identity and creates a new QUIC client; appears as staked peer to TPU
// note: ATM the provided identity might or might not be a valid validator keypair
async fn new_endpoint_with_validator_identity(validator_identity: ValidatorIdentity) -> Endpoint {
    info!(
        "Setup TPU Quic stable connection with validator identity {} ...",
        validator_identity
    );
    // the counterpart of this function is get_remote_pubkey+get_pubkey_from_tls_certificate
    let (certificate, key) = new_self_signed_tls_certificate(
        &validator_identity.get_keypair_for_tls(),
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
    )
    .expect("Failed to initialize QUIC connection certificates");

    create_tpu_client_endpoint(certificate, key)
}

fn create_tpu_client_endpoint(
    certificate: rustls::Certificate,
    key: rustls::PrivateKey,
) -> Endpoint {
    let mut endpoint = {
        let client_socket =
            solana_net_utils::bind_in_range(IpAddr::V4(Ipv4Addr::UNSPECIFIED), (8000, 10000))
                .expect("create_endpoint bind_in_range")
                .1;
        let config = EndpointConfig::default();
        quinn::Endpoint::new(config, None, client_socket, TokioRuntime)
            .expect("create_endpoint quinn::Endpoint::new")
    };

    let mut crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_single_cert(vec![certificate], key)
        .expect("Failed to set QUIC client certificates");

    crypto.enable_early_data = true;

    crypto.alpn_protocols = vec![ALPN_TPU_PROTOCOL_ID.to_vec()];

    let mut config = ClientConfig::new(Arc::new(crypto));

    // note: this should be aligned with solana quic server's endpoint config
    let mut transport_config = TransportConfig::default();
    // no remotely-initiated streams required
    transport_config.max_concurrent_uni_streams(VarInt::from_u32(0));
    transport_config.max_concurrent_bidi_streams(VarInt::from_u32(0));
    let timeout = IdleTimeout::try_from(Duration::from_millis(QUIC_MAX_TIMEOUT_MS as u64)).unwrap();
    transport_config.max_idle_timeout(Some(timeout));
    transport_config.keep_alive_interval(None);

    config.transport_config(Arc::new(transport_config));

    endpoint.set_default_client_config(config);

    endpoint
}

// send potentially large amount of transactions to a single TPU
#[tracing::instrument(skip_all, level = "debug")]
async fn send_tx_batch_to_tpu(auto_connection: &AutoReconnect, txs: &[VersionedTransaction]) {
    for chunk in txs.chunks(MAX_PARALLEL_STREAMS) {
        let all_send_fns = chunk
            .iter()
            .map(|tx| bincode::serialize(tx).unwrap())
            .map(|tx_raw| {
                auto_connection.send_uni(tx_raw) // ignores error
            });

        join_all(all_send_fns).await;
    }
}
