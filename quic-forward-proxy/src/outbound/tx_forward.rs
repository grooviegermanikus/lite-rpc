use crate::outbound::debouncer::Debouncer;
use crate::outbound::sharder::Sharder;
use crate::quic_util::SkipServerVerification;
use crate::quinn_auto_reconnect::AutoReconnect;
use crate::shared::ForwardPacket;
use crate::util::timeout_fallback;
use crate::validator_identity::ValidatorIdentity;
use anyhow::{bail, Context};
use futures::future::join_all;
use log::{debug, info, trace, warn};
use quinn::{
    ClientConfig, Endpoint, EndpointConfig, IdleTimeout, TokioRuntime, TransportConfig, VarInt,
};
use solana_sdk::quic::QUIC_MAX_TIMEOUT;
use solana_streamer::nonblocking::quic::{ALPN_TPU_PROTOCOL_ID, ConnectionPeerType};
use solana_streamer::tls_certificates::new_self_signed_tls_certificate;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::mpsc::Receiver;
use tokio::sync::RwLock;
use solana_lite_rpc_core::quic_connection_utils::QuicConnectionParameters;
use solana_lite_rpc_core::solana_utils::SerializableTransaction;
use solana_lite_rpc_core::stores::data_cache::DataCache;
use solana_lite_rpc_core::structures::identity_stakes::IdentityStakesData;
use solana_lite_rpc_core::structures::transaction_sent_info::SentTransactionInfo;
use solana_lite_rpc_services::tpu_utils::tpu_connection_manager::TpuConnectionManager;

const MAX_PARALLEL_STREAMS: usize = 6;
pub const PARALLEL_TPU_CONNECTION_COUNT: usize = 4;
const AGENT_SHUTDOWN_IDLE: Duration = Duration::from_millis(2500); // ms; should be 4x400ms+buffer

const QUIC_CONNECTION_PARAMS: QuicConnectionParameters = QuicConnectionParameters {
    connection_timeout: Duration::from_secs(2),
    connection_retry_count: 10,
    finalize_timeout: Duration::from_secs(2),
    max_number_of_connections: 8,
    unistream_timeout: Duration::from_secs(2),
    write_timeout: Duration::from_secs(2),
    number_of_transactions_per_unistream: 10,
};


struct AgentHandle {
    pub tpu_address: SocketAddr,
    pub agent_exit_signal: Arc<AtomicBool>,
    pub last_used_at: Arc<RwLock<Instant>>,
}

impl AgentHandle {
    pub async fn touch(&self) {
        let mut timestamp = self.last_used_at.write().await;
        *timestamp = Instant::now();
    }
}

// takes transactions from upstream clients and forwards them to the TPU
pub async fn tx_forwarder(
    validator_identity: ValidatorIdentity,
    mut transaction_channel: Receiver<ForwardPacket>,
    exit_signal: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    info!("TPU Quic forwarder started");

    // let (sender, _) = tokio::sync::broadcast::channel(config.maximum_transaction_in_queue);
    let (sender, _) =
        tokio::sync::broadcast::channel::<SentTransactionInfo>(1000);
    let broadcast_sender = Arc::new(sender);


    // TODO
    let fanout_slots = 4;

    let (certificate, key) = new_self_signed_tls_certificate(
        &validator_identity.get_keypair_for_tls(),
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
    )
        .expect("Failed to initialize QUIC connection certificates");

    // TODO make copy of TpuConnectionManager in proxy crate an strip unused features
    let tpu_connection_manager =
        TpuConnectionManager::new(certificate, key, fanout_slots as usize).await;




    let endpoint = new_endpoint_with_validator_identity(validator_identity).await;

    let (broadcast_in, _) = tokio::sync::broadcast::channel::<Arc<ForwardPacket>>(1024);

    let mut agents: HashMap<SocketAddr, AgentHandle> = HashMap::new();
    let agent_shutdown_debouncer = Debouncer::new(Duration::from_millis(200));

    // TODO implement cleanup
    let mut connections_to_keep: HashMap<Pubkey, SocketAddr> = HashMap::new();

    loop {
        if exit_signal.load(Ordering::Relaxed) {
            bail!("exit signal received");
        }

        let forward_packet = Arc::new(
            transaction_channel
                .recv()
                .await
                .expect("channel closed unexpectedly"),
        );

        // TODO remove
        let identity_stakes = IdentityStakesData {
            peer_type: ConnectionPeerType::Staked,
            stakes: 30,
            min_stakes: 0,
            max_stakes: 40,
            total_stakes: 100,
        };

        connections_to_keep.insert(forward_packet.tpu_identity, forward_packet.tpu_address);
        println!("connections_to_keep: {:?}", connections_to_keep.len());

        // TODO optimize
        tpu_connection_manager
            .update_connections(
                broadcast_sender.clone(),
                &connections_to_keep,
                identity_stakes,
                DataCache::new_for_tests(),
                QUIC_CONNECTION_PARAMS, // TODO improve
            )
            .await;

        for raw_tx in &forward_packet.transactions {

            // TODO add to ForwardPacket
            let tx = bincode::deserialize::<VersionedTransaction>(&raw_tx).unwrap();

            let tsi = SentTransactionInfo {
                signature: tx.get_signature().to_string(),
                slot: 4242,
                transaction: raw_tx.clone(),
                last_valid_block_height: 999,
            };

            // send_transaction
            broadcast_sender.send(tsi).unwrap();

        } // all txs in packet



    } // -- loop over transactions from upstream channels

    // not reachable
}

async fn cleanup_agents(
    agents: &mut HashMap<SocketAddr, AgentHandle>,
    current_tpu_address: &SocketAddr,
) {
    let now = Instant::now();
    let mut to_shutdown = Vec::new();
    for (tpu_address, handle) in &*agents {
        if tpu_address == current_tpu_address {
            continue;
        }

        let unused_period = {
            let last_used_at = handle.last_used_at.read().await;
            now - *last_used_at
        };

        if unused_period > AGENT_SHUTDOWN_IDLE {
            to_shutdown.push(tpu_address.to_owned())
        }
    }

    for tpu_address in to_shutdown.iter() {
        if let Some(removed_agent) = agents.remove(tpu_address) {
            let was_signaled = removed_agent
                .agent_exit_signal
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok();
            if was_signaled {
                let unused_period = {
                    let last_used_ts = removed_agent.last_used_at.read().await;
                    Instant::now() - *last_used_ts
                };
                debug!(
                    "Idle Agent for tpu node {} idle for {}ms - sending exit signal",
                    removed_agent.tpu_address,
                    unused_period.as_millis()
                );
            }
        }
    }
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
    let timeout = IdleTimeout::try_from(QUIC_MAX_TIMEOUT).unwrap();
    transport_config.max_idle_timeout(Some(timeout));
    transport_config.keep_alive_interval(Some(Duration::from_millis(500)));

    config.transport_config(Arc::new(transport_config));

    endpoint.set_default_client_config(config);

    endpoint
}

// send potentially large amount of transactions to a single TPU
#[tracing::instrument(skip_all, level = "debug")]
async fn send_tx_batch_to_tpu(auto_connection: &AutoReconnect, txs: &[Vec<u8>]) {
    for chunk in txs.chunks(MAX_PARALLEL_STREAMS) {
        let all_send_fns = chunk.iter().map(|tx_raw| {
            auto_connection.send_uni(tx_raw) // ignores error
        });

        join_all(all_send_fns).await;
    }
}
