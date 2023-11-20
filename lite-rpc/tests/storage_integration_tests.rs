use std::backtrace::Backtrace;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::mem::size_of;
use std::panic::PanicInfo;
use std::process;
use std::str::FromStr;
use jsonrpsee::tracing::warn;
use log::{debug, error, info};
use solana_lite_rpc_cluster_endpoints::endpoint_stremers::EndpointStreaming;
use solana_lite_rpc_cluster_endpoints::json_rpc_subscription::create_json_rpc_polling_subscription;
use solana_lite_rpc_core::structures::epoch::EpochCache;
use solana_lite_rpc_core::structures::produced_block::{ProducedBlock, TransactionInfo};
use solana_lite_rpc_core::traits::block_storage_interface::BlockStorageInterface;
use solana_lite_rpc_core::types::BlockStream;
use solana_lite_rpc_history::block_stores::postgres_block_store::PostgresBlockStore;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use chrono::format::format_item;
use countmap::CountMap;
use csv::WriterBuilder;
use fxhash::FxHasher32;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use solana_sdk::blake3::{hash, Hash, HASH_BYTES};
use solana_sdk::clock::Slot;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::shred_version::compute_shred_version;
use solana_sdk::signature::Signature;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format;

#[tokio::test]
async fn storage_test() {
    // RUST_LOG=info,storage_integration_tests=debug,solana_lite_rpc_history=trace
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    configure_panic_hook();

    let rpc_url = std::env::var("RPC_URL").expect("env var RPC_URL is mandatory");

    let rpc_client = Arc::new(RpcClient::new(rpc_url));

    let (subscriptions, _cluster_endpoint_tasks) =
        create_json_rpc_polling_subscription(rpc_client.clone()).unwrap();

    let EndpointStreaming {
        blocks_notifier, ..
    } = subscriptions;

    let epoch_data = EpochCache::bootstrap_epoch(&rpc_client).await.unwrap();

    // let block_storage = Arc::new(PostgresBlockStore::new(epoch_data).await);

    // let jh1 = storage_listen(blocks_notifier.resubscribe(), block_storage.clone());
    // let jh2 = block_debug_listen(blocks_notifier.resubscribe());
    // let jh3 = block_stream_assert_commitment_order(blocks_notifier.resubscribe());
    let jh4 = compress_account_ids(blocks_notifier.resubscribe());
    drop(blocks_notifier);

    info!("Run tests for some time ...");
    sleep(Duration::from_secs(1200)).await;

    // jh1.abort();
    // jh2.abort();
    // jh3.abort();
    jh4.abort();

    info!("Tests aborted forcefully by design.");
}

// note: the consumer lags far behind the ingress of blocks and transactions
fn storage_listen(
    block_notifier: BlockStream,
    block_storage: Arc<PostgresBlockStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut block_notifier = block_notifier;
        loop {
            match block_notifier.recv().await {
                Ok(block) => {
                    debug!(
                        "Received block: {} with {} txs",
                        block.slot,
                        block.transactions.len()
                    );

                    // TODO we should intercept finalized blocks and try to update only the status optimistically

                    let produced_block = ProducedBlock {
                        transactions: block.transactions,
                        leader_id: block.leader_id,
                        blockhash: block.blockhash,
                        block_height: block.block_height,
                        slot: block.slot,
                        parent_slot: block.parent_slot,
                        block_time: block.block_time,
                        commitment_config: block.commitment_config,
                        previous_blockhash: block.previous_blockhash,
                        rewards: block.rewards,
                    };

                    let started = Instant::now();
                    // avoid backpressure here!
                    // TODO check timing
                    block_storage.save(&produced_block).await.unwrap();
                    // we should be faster than 150ms here
                    debug!(
                        "Saving block to postgres took {:.2}ms",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                } // -- Ok
                Err(RecvError::Lagged(missed_blocks)) => {
                    warn!(
                        "Could not keep up with producer - missed {} blocks",
                        missed_blocks
                    );
                }
                Err(other_err) => {
                    panic!("Error receiving block: {:?}", other_err);
                }
            }

            // ...
        }
    })
}

#[derive(Debug, Clone)]
struct BlockDebugDetails {
    pub blockhash: String,
    pub block: ProducedBlock,
}


#[derive(serde::Serialize)]
struct CsvRow<'a> {
    account_pubkey: &'a str,
    hash: u32,
    hash2: u32,
}

fn compress_account_ids(block_notifier: BlockStream) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut block_notifier = block_notifier;

        // 1 GB RAM for the two arrays
        let mut collisions = vec![0_u8;  u32::MAX as usize / 8];
        let mut collisions2 = vec![0_u8;  u32::MAX as usize / 8];

        info!("ALLOCATED ALOT OF MEM");

        const USE_FULL_COLLISSION_MAP: bool = false;
        let mut seen = HashMap::<u32, Pubkey>::new();

        let mut count_by_key = CountMap::<Pubkey>::new();

        let mut csv_writer = WriterBuilder::new().from_path(format!("collissions.csv")).unwrap();
        let mut slots = HashSet::new();
        let mut num_accounts: Vec<usize> = Vec::new();

        loop {
            match block_notifier.recv().await {
                Ok(block) => {
                    debug!(
                        "Saw block: {} @ {} with {} txs",
                        block.slot,
                        block.commitment_config.commitment,
                        block.transactions.len()
                    );

                    slots.insert(block.slot);

                    let started = Instant::now();
                    let mut count = 0;
                    for tx in &block.transactions {
                        if is_vote(&tx) {
                            continue;
                        }
                        
                        info!("tx {}", tx.signature);
                        num_accounts.push(tx.static_account_keys.len());

                        for acc in &tx.static_account_keys {
                            info!("- {}", acc);
                            let hash: u32 = hash32(&acc);
                            let hash2: u32 = hash32_check(&acc);
                            count_by_key.insert_or_increment(acc.clone());

                            csv_writer.serialize(CsvRow {
                                account_pubkey: acc.to_string().as_str(),
                                hash,
                                hash2,
                            }).unwrap();

                            let hash_used_before = collisions[hash as usize / 8] & (1 << (hash % 8)) != 0;
                            if hash_used_before {
                                if collisions2[hash2 as usize / 8] & (1 << (hash2 % 8)) == 0 {
                                    info!("optimistic collision check for {}", acc);
                                    info!("slots mapped without collision: {:?}", slots.len());
                                    info!("slots mapped without collision: {:?}", slots);
                                    csv_writer.flush().unwrap();
                                    panic!("collision for {}", acc);
                                }
                            }

                            if USE_FULL_COLLISSION_MAP {
                                let inserted = seen.insert(hash, acc.clone());
                                if let Some(ref dupe) = inserted {
                                    if dupe != acc {
                                        panic!("collision hash key {}: {} -> {}", hash, acc, dupe);
                                    }
                                }
                            }

                            collisions2[hash2 as usize / 8] |= 1 << (hash2 % 8);
                            collisions[hash as usize / 8] |= 1 << (hash % 8);


                            // failes with 436/524 (at 16 bit)
                            // if count % 1000 == 0 {
                            //     info!("ones: {}", count_ones(collisions.as_ref()));
                            //     info!("ones2: {}", count_ones(collisions2.as_ref()));
                            // }

                            count += 1;

                            if count % 1000 == 0 {
                                info!("Distinct account seen so far: {}", count_by_key.len());

                                let (sum,cnt) = num_accounts.iter().fold((0, 0), |acc, &n| {
                                    (acc.0 + n, acc.1 + 1)
                                });
                                // 10.2
                                info!("Avg size of account set: {}", sum as f64 / cnt as f64);

                            }

                            if slots.len() == 1000 {
                                info!("Compress {} slots", slots.len());
                                let mut matrix = MatrixCompressionChunk::new();
                                let sig = Pubkey::from_str(&tx.signature).unwrap();
                                matrix.insert(&sig, &tx.static_account_keys);


                            }


                        }
                    } // -- all txs in block

                    info!("hashing took {:.2}ms for {} keys", started.elapsed().as_secs_f64() * 1000.0, count);
                    // info!("ones: {}", count_ones(collisions.as_ref()));

                } // -- Ok
                Err(RecvError::Lagged(missed_blocks)) => {
                    warn!(
                        "Could not keep up with producer - missed {} blocks",
                        missed_blocks
                    );
                }
                Err(other_err) => {
                    panic!("Error receiving block: {:?}", other_err);
                }
            }

            // ...
        }
    })
}

// n txs x m distinct accounts
#[derive(Serialize)]
struct MatrixCompressionChunk {
    txs: Vec<CompressedTransaction>,
    // TODO is u32 enough?
    global_collision_protection: Vec<u32>,
}

impl MatrixCompressionChunk {
    pub fn new() -> Self {
        Self {
            txs: Vec::new(),
            global_collision_protection: Vec::new(),
        }
    }
    pub fn is_in_domain(&self, account_key: &Pubkey) -> bool {
        let gh = hash32_check(&account_key);
        self.global_collision_protection.binary_search(&gh).is_ok()
    }
    pub fn insert(&mut self, tx_sig: &Pubkey, account_keys: &[Pubkey]) {

        let sorted_hashed_accounts = account_keys.iter().map(|acc| hash32(&acc)).sorted().collect_vec();

        info!("sorted_hashed_accounts= {:?}", sorted_hashed_accounts);

        self.txs.push(CompressedTransaction {
            tx_sig: tx_sig.clone(),
            acc_compressed: sorted_hashed_accounts,
        });

        for account_key in account_keys {
            let gh = hash32_check(&account_key);
            let matching = self.global_collision_protection.binary_search(&gh);
            match matching {
                Err(insertion_idx) => {
                    self.global_collision_protection.insert(insertion_idx, gh);
                }
                Ok(_) => continue,
            }
        }

    }

    pub fn get_signatures_for_address(&self, account_key: &Pubkey) -> Vec<Pubkey> {
        if !self.is_in_domain(account_key) {
            // account pub key not mapped
            return vec![];
        }

        let hash = hash32(account_key);
        let signatures =
            self.txs.iter()
                .filter(|tx| tx.has_account_by_hash(hash))
                .map(|tx| tx.tx_sig.clone())
                .collect_vec();

        return signatures;
    }
}

#[derive(Serialize)]
struct CompressedTransaction {
    tx_sig: Pubkey,
    // must be sorted!
    acc_compressed: Vec<u32>,
}

impl CompressedTransaction {
    pub fn has_account(&self, acc: &Pubkey) -> bool {
        let hash = hash32(acc);
        self.acc_compressed.binary_search(&hash).is_ok()
    }
    pub fn has_account_by_hash(&self, account_hash: u32) -> bool {
        self.acc_compressed.binary_search(&account_hash).is_ok()
    }
}

#[test]
fn insert_into_matrix() {
    tracing_subscriber::fmt::init();

    let mut matrix = MatrixCompressionChunk::new();
    let tx_sig = Pubkey::from_str("Bm8rtweCQ19ksNebrLY92H7x4bCaeDJSSmEeWqkdCeop").unwrap();
    let account1 = Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ").unwrap();
    let account2 = Pubkey::from_str("1111111k4AYMctpyJakWNvGcte6tR8BLyZw54R8qu").unwrap();

    matrix.insert(&tx_sig, &[account1, account2]);

}


#[test]
fn gsfa() {
    tracing_subscriber::fmt::init();

    let mut matrix = MatrixCompressionChunk::new();

    let account1 = Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ").unwrap();
    let account2 = Pubkey::from_str("1111111k4AYMctpyJakWNvGcte6tR8BLyZw54R8qu").unwrap();
    let account3 = Pubkey::from_str("1111111k4AYMctpyJakWNvGcte6tR8BLyZw54R8qq").unwrap();

    let tx1_sig = Pubkey::from_str("Bm8rtweCQ19ksNebrLY92H7x4bCaeDJSSmEeWqkdCeop").unwrap();
    matrix.insert(&tx1_sig, &[account1, account2]);
    let tx2_sig = Pubkey::from_str("Bm8rtweCQ19ksNebrLY92H7x4bCaeDJSSmEeWqkdCeoq").unwrap();
    matrix.insert(&tx2_sig, &[account1]);

    for _i in 0..100 {
        let tx_rnd = Pubkey::new_unique();
        matrix.insert(&tx_rnd, &[account3]);
    }

    let found = matrix.get_signatures_for_address(&Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ").unwrap());
    assert_eq!(2, found.len());

    let found = matrix.get_signatures_for_address(&Pubkey::from_str("1111111k4AYMctpyJakWNvGcte6tR8BLyZw54R8qu").unwrap());
    assert_eq!(1, found.len());

    let json = serde_json::to_string(&matrix).unwrap();

    info!("json {}", json);

    assert_eq!(4520, bincode::serialize(&matrix).unwrap().len());

    // 44 bytes each
    // sig: 32 bytes
    // each account costs 4 bytes
    for _i in 0..1000 {
        let tx_rnd = Pubkey::new_unique();
        matrix.insert(&tx_rnd, &[account3]);
    }

    assert_eq!(48520, bincode::serialize(&matrix).unwrap().len());
}


fn is_vote(tx: &TransactionInfo) -> bool {
    let vote_account = Pubkey::from_str("Vote111111111111111111111111111111111111111").unwrap();
    let maybe_vote = tx.static_account_keys.get(2);
    maybe_vote.map(|acc| acc == &vote_account).unwrap_or(false)
}

#[inline]
fn hash16(p0: &Pubkey) -> u16 {
    fxhash::hash32(p0.as_ref()) as u16
}

#[inline]
fn hash16_check(p0: &Pubkey) -> u16 {
    fxhash::hash32(p0.as_ref()) as u16
}

#[inline]
fn hash32(p0: &Pubkey) -> u32 {
    let mut hasher = FxHasher32::default();
    hasher.write_u64(0x9911223344001133);
    hasher.write(p0.as_ref());
    hasher.finish() as u32
}

#[inline]
fn hash32_check(p0: &Pubkey) -> u32 {
    let mut hasher = FxHasher32::default();
    hasher.write_u64(0x2348012701123212);
    hasher.write(p0.as_ref());
    hasher.finish() as u32
}

fn count_ones(data: &[u8]) -> u32 {
    let mut count = 0;
    for byte in data {
        count += byte.count_ones();
    }
    count
}


#[test]
pub fn hash_to_16bit() {
    let account_key = Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ").unwrap();

    assert_eq!(28038u16, hash16(&account_key));

}

#[test]
pub fn hash_to_32bit() {
    let account_key = Pubkey::from_str("1111111jepwNWbYG87sgwnBbUJnQHrPiUJzMpqJXZ").unwrap();

    assert_eq!(1779384094, hash32(&account_key));
    assert_eq!(1260434961u32, hash32_check(&account_key));

}

#[test]
fn cast16() {
    assert_eq!(34464, 100000u32 as u16);
}

fn block_debug_listen(block_notifier: BlockStream) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_highest_slot_number = 0;
        let mut block_notifier = block_notifier;

        loop {
            match block_notifier.recv().await {
                Ok(block) => {
                    debug!(
                        "Saw block: {} @ {} with {} txs",
                        block.slot,
                        block.commitment_config.commitment,
                        block.transactions.len()
                    );

                    // check monotony
                    // note: this succeeds if poll_block parallelism is 1 (see NUM_PARALLEL_BLOCKS)
                    if block.commitment_config == CommitmentConfig::confirmed() {
                        if block.slot > last_highest_slot_number {
                            last_highest_slot_number = block.slot;
                        } else {
                            // note: ATM this fails very often (using the RPC poller)
                            warn!(
                                "Monotonic check failed - block {} is out of order, last highest was {}",
                                block.slot, last_highest_slot_number
                            );
                        }
                    }

                } // -- Ok
                Err(RecvError::Lagged(missed_blocks)) => {
                    warn!(
                        "Could not keep up with producer - missed {} blocks",
                        missed_blocks
                    );
                }
                Err(other_err) => {
                    panic!("Error receiving block: {:?}", other_err);
                }
            }

            // ...
        }
    })
}

/// inspect stream of blocks and check that the commitment transition from confirmed to finalized is correct
fn block_stream_assert_commitment_order(block_notifier: BlockStream) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut block_notifier = block_notifier;

        let mut confirmed_blocks_by_slot = HashMap::<Slot, BlockDebugDetails>::new();
        let mut finalized_blocks = HashSet::<Slot>::new();

        let mut warmup_cutoff: Slot = 0;
        let mut warmup_first_confirmed: Slot = 0;

        loop {
            match block_notifier.recv().await {
                Ok(block) => {
                    if warmup_cutoff > 0 {
                        if block.slot < warmup_cutoff {
                            continue;
                        }

                        // check semantics and log/panic
                        inspect_this_block(&mut confirmed_blocks_by_slot, &mut finalized_blocks, &block);
                    } else {
                        debug!("Warming up {} ...", block.slot);

                        if warmup_first_confirmed == 0 && block.commitment_config == CommitmentConfig::confirmed() {
                            warmup_first_confirmed = block.slot;
                        }

                        if block.commitment_config == CommitmentConfig::finalized() {
                            if block.slot >= warmup_first_confirmed {
                                warmup_cutoff = block.slot + 32;
                                debug!("Warming done (slot {})", warmup_cutoff);

                            }
                        }

                    }

                } // -- Ok
                Err(RecvError::Lagged(missed_blocks)) => {
                    warn!(
                        "Could not keep up with producer - missed {} blocks",
                        missed_blocks
                    );
                }
                Err(other_err) => {
                    panic!("Error receiving block: {:?}", other_err);
                }
            }

            // ...
        }
    })
}

fn inspect_this_block(confirmed_blocks_by_slot: &mut HashMap<Slot, BlockDebugDetails>, finalized_blocks: &mut HashSet<Slot>, block: &ProducedBlock) {
    if block.commitment_config == CommitmentConfig::confirmed() {
        let prev_block = confirmed_blocks_by_slot.insert(block.slot, BlockDebugDetails {
            blockhash: block.blockhash.clone(),
            block: block.clone(),
        });
        // Assumption I: we never see the same confirmed block twice
        assert!(prev_block.is_none(), "Must not see a confirmed block twice");
    } else if block.commitment_config == CommitmentConfig::finalized() {
        let finalized_block = &block;
        let finalized_block_existed = finalized_blocks.insert(finalized_block.slot);
        // Assumption II: we never see the same finalized block twice
        assert!(finalized_block_existed, "Finalized block {} must NOT have been seen before", finalized_block.slot);
        let prev_block = confirmed_blocks_by_slot.get(&block.slot);
        match prev_block {
            Some(prev_block) => {
                info!("Got finalized block {} with blockhash {} - prev confirmed was {}",
                                        finalized_block.slot, finalized_block.blockhash, prev_block.blockhash);
                // TODO is that correct?
                // Assumption III: confirmed and finalized block can be matched by slot and have the same blockhash
                assert_eq!(finalized_block.blockhash, prev_block.blockhash, "Must see the same blockhash for confirmed and finalized block");

                debug!("confirmed: {:?}", to_string_without_transactions(&prev_block.block));
                debug!("finalized: {:?}", to_string_without_transactions(&finalized_block));

                // Assumption IV: block details do not change between confirmed and finalized
                assert_eq!(
                    // normalized and compare
                    to_string_without_transactions(&prev_block.block).replace("commitment_config=confirmed", "commitment_config=IGNORE"),
                    to_string_without_transactions(&finalized_block).replace("commitment_config=finalized", "commitment_config=IGNORE"),
                    "block tostring mismatch"
                )
            }
            None => {
                // note at startup we might see some orphan finalized blocks before we see matching pairs of confirmed-finalized blocks
                panic!("Must see a confirmed block before it is finalized (slot {}) - could be a warmup issue", finalized_block.slot);
            }
        }
    }
}

fn to_string_without_transactions(produced_block: &ProducedBlock) -> String {
    format!(
        r#"
            leader_id={:?}
            blockhash={}
            block_height={}
            slot={}
            parent_slot={}
            block_time={}
            commitment_config={}
            previous_blockhash={}
            num_transactions={}
        "#,
        produced_block.leader_id,
        produced_block.blockhash,
        produced_block.block_height,
        produced_block.slot,
        produced_block.parent_slot,
        produced_block.block_time,
        produced_block.commitment_config.commitment,
        produced_block.previous_blockhash,
        produced_block.transactions.len(),
        // rewards
        // transactions
    )
}

fn configure_panic_hook() {
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        default_panic(panic_info);
        // e.g. panicked at 'BANG', lite-rpc/tests/storage_integration_tests.rs:260:25
        error!("{}", panic_info);
        eprintln!("{}", panic_info);
        process::exit(12);
    }));
}
