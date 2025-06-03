use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};
use crossbeam_channel;
use log::{debug, error, info, trace, warn};
use regex::{Captures, Regex};
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};
use solana_sdk::transaction::{Transaction, VersionedTransaction};
use solana_streamer::nonblocking::quic::{ConnectionPeerType, SpawnNonBlockingServerResult};
use solana_streamer::packet::PacketBatch;
use solana_streamer::quic::{QuicServerParams, StreamerStats};
use solana_streamer::streamer::StakedNodes;
use solana_tls_utils::new_dummy_x509_certificate;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Instant};
use solana_lite_rpc_core::solana_utils::SerializableTransaction;
use solana_lite_rpc_core::stores::data_cache::DataCache;
use solana_lite_rpc_core::structures::identity_stakes::IdentityStakesData;
use solana_lite_rpc_core::structures::produced_block::TransactionInfo;
use solana_lite_rpc_core::structures::transaction_sent_info::SentTransactionInfo;
use solana_lite_rpc_services::quic_connection_utils::QuicConnectionParameters;
use solana_lite_rpc_services::tpu_utils::tpu_connection_manager::TpuConnectionManager;


const MAX_QUIC_CONNECTIONS_PER_PEER: usize = 8; // like solana repo

const QUIC_CONNECTION_PARAMS: QuicConnectionParameters = QuicConnectionParameters {
    connection_timeout: Duration::from_secs(2),
    connection_retry_count: 10,
    finalize_timeout: Duration::from_secs(2),
    max_number_of_connections: 8,
    unistream_timeout: Duration::from_secs(2),
    write_timeout: Duration::from_secs(2),
    number_of_transactions_per_unistream: 10,
    unistreams_to_create_new_connection_in_percentage: 10,
    prioritization_heap_size: None,
};

#[derive(Copy, Clone, Debug)]
struct TestCaseParams {
    sample_tx_count: u32,
    stake_connection: bool,
    proxy_mode: bool,
}

async fn wire_up() -> anyhow::Result<()> {
    configure_panic_hook();

    let test_case_params = TestCaseParams {
        sample_tx_count: 10,
        stake_connection: true,
        proxy_mode: false,
    };

    // CAUTION: this is a blocking channel
    let (sender, tx_from_streamer_receiver) = crossbeam_channel::unbounded::<PacketBatch>();

    let literpc_validator_identity = Arc::new(Keypair::new());

    let (streamer_listen_address) = spawn_streamer(
        test_case_params,
        literpc_validator_identity.clone(),
        sender,
    )?;

    let (tx, mut rx) = mpsc::channel::<PacketBatch>(100_000);

    start_crossbeam_bridge_thread(tx_from_streamer_receiver,tx);

    let broadcast_sender = spawn_literpc_client_direct_mode(
        test_case_params,
        streamer_listen_address,
        literpc_validator_identity)
        .await?;

    // race - wait for the literpc component
    // sleep(Duration::from_secs(2)).await;

    let send_tx_broadcast = broadcast_sender.clone();
    tokio::spawn(async move {
        let broadcast_start_at = Instant::now();
        // note: this must fast and should not block
        for i in 0..test_case_params.sample_tx_count {
            let raw_sample_tx = build_raw_sample_tx(i);
            trace!(
                "broadcast transaction {}: {}",
                raw_sample_tx.signature,
                format!("hi {}", i)
            );
            send_tx_broadcast.send(raw_sample_tx).unwrap();
        }
        debug!(
            "broadcasted {} transactions in {:?}ms",
            test_case_params.sample_tx_count,
            broadcast_start_at.elapsed().as_secs_f64() * 1000.0
        );
    });

    // race2 - let the system soak up the transactions
    sleep(Duration::from_secs(2)).await;

    info!("Everything is in place, starting to receive packets from streamer...");

    let mut received_messages: HashSet<u64> = HashSet::new();
    'tx_from_streamer_loop: loop {
        use tokio::time::timeout;

        match timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(packet_batch)) => {
                info!("received packet batch with {} packets", packet_batch.len());
                for packet in packet_batch.iter() {
                    let tx = packet
                        .deserialize_slice::<VersionedTransaction, _>(..)
                        .unwrap();
                    let content = String::from_utf8(tx.message.instructions()[0].data.clone()).unwrap();
                    // "hi 1 @1748886442000"
                    let (tx_counter, _epoch_us) = parse_hi(&content).unwrap();
                    let was_inserted = received_messages.insert(tx_counter);
                    assert!(was_inserted,
                        "Received duplicate transaction {}: {}",
                        tx.get_signature(),
                        content);
                }
            }
            Ok(None) => {
                warn!("rx channel from streamer closed");
                unreachable!();
            }
            Err(_timeout) => {
                info!("txs received: {}", received_messages.len());
                // TODO check if we have received all transactions
                warn!("Timeout while waiting for packet batch");
                break 'tx_from_streamer_loop;
            }
        }
    }
    assert_eq!(
        broadcast_sender.len(), 0,
        "broadcast channel must be empty"
    );

    // evaluate received messagesd
    if received_messages.len() != test_case_params.sample_tx_count as usize {
        assert_eq!(
            received_messages.len() as u32,
            test_case_params.sample_tx_count,
            "Must have received all transactions"
        );
    }

    info!("Received all {} transactions", test_case_params.sample_tx_count);

    Ok(())
}

fn spawn_streamer(test_case_params: TestCaseParams,
                  literpc_validator_identity: Arc<Keypair>,
                  inbound_packets_sender: crossbeam_channel::Sender<PacketBatch>) -> anyhow::Result<(SocketAddr)> {

    let udp_listen_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let listen_addr = udp_listen_socket.local_addr().unwrap();

    // setup solana Quic streamer
    // see log "Start quic server on UdpSocket { addr: 127.0.0.1:xxxxx, fd: 10 }"
    let stakes_map = Arc::new(if test_case_params.stake_connection {
        let mut map = HashMap::new();
        map.insert(literpc_validator_identity.pubkey(), 30);
        map
    } else {
        HashMap::default()
    });
    let staked_nodes = StakedNodes::new(stakes_map, Default::default());

    let _solana_quic_streamer = SolanaQuicStreamer::new_start_listening(
        udp_listen_socket,
        inbound_packets_sender,
        staked_nodes,
    );

    Ok((listen_addr))

}


// no quic proxy
async fn spawn_literpc_client_direct_mode(
    test_case_params: TestCaseParams,
    streamer_listen_addrs: SocketAddr,
    literpc_validator_identity: Arc<Keypair>,
) -> anyhow::Result<Arc<broadcast::Sender<SentTransactionInfo>>> {
    info!("Start lite-rpc test client in direct-mode...");

    // not used ATM
    let fanout_slots = 4;

    let (sender, _) = tokio::sync::broadcast::channel(10_000);
    let broadcast_sender = Arc::new(sender);
    let (certificate, key) = new_dummy_x509_certificate(literpc_validator_identity.as_ref());

    let tpu_connection_manager =
        TpuConnectionManager::new(certificate, key, fanout_slots as usize).await;

    // this effectively controls how many connections we will have
    let mut connections_to_keep: HashMap<Pubkey, SocketAddr> = HashMap::new();
    let addr1 = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    connections_to_keep.insert(
        Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ")?,
        addr1,
    );

    let addr2 = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    connections_to_keep.insert(
        Pubkey::from_str("1111111k4AYMctpyJakWNvGcte6tR8BLyZw54R8qu")?,
        addr2,
    );

    // this is the real streamer
    connections_to_keep.insert(literpc_validator_identity.pubkey(), streamer_listen_addrs);

    // get information about the optional validator identity stake
    // populated from get_stakes_for_identity()
    let stakes = if test_case_params.stake_connection {
        30
    } else {
        0
    };
    let identity_stakes = IdentityStakesData {
        peer_type: ConnectionPeerType::Staked(stakes),
        stakes, // stake of lite-rpc
        min_stakes: 0,
        max_stakes: 40,
        total_stakes: 100,
    };

    info!("about to update_connections...");
    tpu_connection_manager
        .update_connections(
            broadcast_sender.clone(),
            connections_to_keep,
            identity_stakes,
            // note: tx_store is useless in this scenario as it is never changed; it's only used to check for duplicates
            DataCache::new_for_tests(),
            QUIC_CONNECTION_PARAMS,
        )
        .await;

    info!("TpuConnectionManager updated connections");

    // keep manager running
    // let send2 = broadcast_sender.clone();
    tokio::spawn(async move {
        let _ = tpu_connection_manager;
        info!("TpuConnectionManager is running...");

        // send2.send(SentTransactionInfo {
        //     signature: Signature::default(),
        //     slot: 0,
        //     transaction: Arc::new(vec![]),
        //     last_valid_block_height: 0,
        //     prioritization_fee: 0,
        // }).unwrap();
        // sleep to avoid busy loop
        sleep(Duration::from_secs(20)).await;
        info!("TpuConnectionManager shutting down after n seconds");
    });

    info!("checkpoint2");

    Ok(broadcast_sender)
}

#[tokio::test]
pub async fn test_simple() {
    let _ = tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::DEBUG).try_init();
    info!("Starting QUIC forward proxy integration test...");

    // make sure that tokio is never blocked
    tokio::spawn(async move {
        // keep the streamer running
        loop {
            info!("some tokio work..");
            sleep(Duration::from_millis(300)).await;
        }
    });

    wire_up().await.unwrap();
}



// TODO remove
#[tokio::test]
pub async fn test_only_streamer_single_thread() {
    let _ = tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::DEBUG).try_init();

    let test_case_params = TestCaseParams {
        sample_tx_count: 10,
        stake_connection: true,
        proxy_mode: false,
    };

    let (sender, tx_from_streamer_receiver) = crossbeam_channel::unbounded::<PacketBatch>();

    let literpc_validator_identity = Arc::new(Keypair::new());

    tokio::spawn(async move {
        // keep the streamer running
        loop {
            info!("some tokio work..");
            sleep(Duration::from_millis(300)).await;
        }
    });

    info!("Starting solana tpu streamer...");
    let (streamer_listen_address) = spawn_streamer(
        test_case_params,
        literpc_validator_identity.clone(),
        sender,
    ).unwrap();

    sleep(Duration::from_secs(2)).await;

    info!("done starting solana tpu streamer...");

}



#[allow(dead_code)]
struct SolanaQuicStreamer {
    sock: UdpSocket,
    exit: Arc<AtomicBool>,
    join_handler: JoinHandle<()>,
    stats: Arc<StreamerStats>,
}

impl SolanaQuicStreamer {
    fn new_start_listening(
        udp_socket: UdpSocket,
        sender: crossbeam_channel::Sender<PacketBatch>,
        staked_nodes: StakedNodes,
    ) -> Self {
        let staked_nodes = Arc::new(RwLock::new(staked_nodes));
        let exit = Arc::new(AtomicBool::new(false));
        // keypair to derive the server tls certificate
        let keypair = Keypair::new();

        let SpawnNonBlockingServerResult { stats, thread, .. } =
            solana_streamer::nonblocking::quic::spawn_server(
                "test-quic-server",
                udp_socket.try_clone().unwrap(),
                &keypair,
                sender,
                exit.clone(),
                staked_nodes,
                QuicServerParams {
                    max_connections_per_peer: MAX_QUIC_CONNECTIONS_PER_PEER,
                    max_staked_connections: 10,
                    max_unstaked_connections: 10,
                    max_streams_per_ms: 9999,
                    max_connections_per_ipaddr_per_min: 10,
                    wait_for_chunk_timeout: Duration::from_millis(8000),
                    coalesce: Duration::from_millis(3),
                    coalesce_channel_size: 1000,
                },
            )
                .unwrap();

        let addr = udp_socket.local_addr().unwrap().ip();
        let port = udp_socket.local_addr().unwrap().port();
        let _tpu_addr = SocketAddr::new(addr, port);

        Self {
            sock: udp_socket,
            exit,
            join_handler: thread,
            stats,
        }
    }
}


const MEMO_PROGRAM_ID: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

pub fn build_raw_sample_tx(i: u32) -> SentTransactionInfo {
    let payer_keypair = Keypair::from_base58_string(
        "rKiJ7H5UUp3JR18kNyTF1XPuwPKHEM7gMLWHZPWP5djrW1vSjfwjhvJrevxF9MPmUmN9gJMLHZdLMgc9ao78eKr",
    );

    let tx = build_sample_tx(&payer_keypair, i);

    let transaction =
        Arc::new(bincode::serialize::<VersionedTransaction>(&tx).expect("failed to serialize tx"));

    SentTransactionInfo {
        signature: *tx.get_signature(),
        slot: 1,
        transaction,
        last_valid_block_height: 300,
        prioritization_fee: 0,
    }
}


fn build_sample_tx(payer_keypair: &Keypair, i: u32) -> VersionedTransaction {
    let blockhash = Hash::default();
    let epoch_us = get_epoch_now_us();
    let epoch_us = epoch_us as u64;
    create_memo_tx(format!("hi {} @{}", i, epoch_us).as_bytes(), payer_keypair, blockhash).into()
}

fn parse_hi(input: &str) -> Option<(u64, u64)> {
    let re = Regex::new(r"hi (\d+) @(\d+)").unwrap();
    let groups: Captures = re.captures(&input).unwrap();
    let counter = groups.get(1).unwrap().as_str().parse::<u64>().ok()?;
    let epoch_us = groups.get(2).unwrap().as_str().parse::<u64>().ok()?;
    Some((counter, epoch_us))
}

fn create_memo_tx(msg: &[u8], payer: &Keypair, blockhash: Hash) -> Transaction {
    let memo = Pubkey::from_str(MEMO_PROGRAM_ID).unwrap();

    let instruction = Instruction::new_with_bytes(memo, msg, vec![]);
    let message = Message::new(&[instruction], Some(&payer.pubkey()));
    Transaction::new(&[payer], message, blockhash)
}

fn get_epoch_now_us() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}


fn configure_panic_hook() {
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        default_panic(panic_info);
        if let Some(location) = panic_info.location() {
            error!(
                "panic occurred in file '{}' at line {}",
                location.file(),
                location.line(),
            );
        } else {
            error!("panic occurred but can't get location information...");
        }
        // note: we do not exit the process to allow proper test execution
    }));
}


fn start_crossbeam_bridge_thread<T: Send + Sync + 'static>(rx: crossbeam_channel::Receiver<T>, tx: mpsc::Sender<T>) {
    std::thread::spawn(move || {
        'forward_loop: loop {
            match rx.recv() {
                Ok(data) => {
                    if let Err(err) = tx.blocking_send(data) {
                        error!("Failed to send data to tokio mpsc: {:?}", err);
                        break 'forward_loop;
                    }
                }
                Err(err) => {
                    warn!("Aborting bridge thread on error receiving from crossbeam channel: {:?}", err);
                    break 'forward_loop;
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        }
    });
}
