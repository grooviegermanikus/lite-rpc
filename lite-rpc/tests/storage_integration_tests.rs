use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use jsonrpsee::tracing::warn;
use log::{debug, error, info};
use solana_lite_rpc_cluster_endpoints::endpoint_stremers::EndpointStreaming;
use solana_lite_rpc_cluster_endpoints::json_rpc_subscription::create_json_rpc_polling_subscription;
use solana_lite_rpc_core::structures::epoch::EpochCache;
use solana_lite_rpc_core::structures::produced_block::ProducedBlock;
use solana_lite_rpc_core::traits::block_storage_interface::BlockStorageInterface;
use solana_lite_rpc_core::types::BlockStream;
use solana_lite_rpc_history::block_stores::postgres_block_store::PostgresBlockStore;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use solana_sdk::clock::Slot;
use solana_sdk::commitment_config::CommitmentConfig;
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

    let block_storage = Arc::new(PostgresBlockStore::new(epoch_data).await);

    let jh1 = storage_listen(blocks_notifier.resubscribe(), block_storage.clone());
    let jh2 = block_debug_listen(blocks_notifier.resubscribe());
    drop(blocks_notifier);

    info!("Run tests for some time ...");
    sleep(Duration::from_secs(20)).await;

    jh1.abort();
    jh2.abort();

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

fn block_debug_listen(block_notifier: BlockStream) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_highest_slot_number = 0;
        let mut block_notifier = block_notifier;

        let mut confirmed_blocks_by_slot = HashMap::<Slot, BlockDebugDetails>::new();
        let mut finalized_blocks = HashSet::<Slot>::new();

        let mut warmup = true;
        let mut warmup_first_confirmed: Slot = 0;

        loop {
            match block_notifier.recv().await {
                Ok(block) => {
                    debug!(
                        "Saw block: {} with {} txs",
                        block.slot,
                        block.transactions.len()
                    );

                    /// check the commitment transition from confirmed to finalized
                    if warmup {
                        debug!("Warming up {} ...", block.slot);

                        if block.commitment_config == CommitmentConfig::confirmed() && warmup_first_confirmed == 0 {
                            warmup_first_confirmed = block.slot;
                        }

                        if block.commitment_config == CommitmentConfig::finalized() {
                            if block.slot >= warmup_first_confirmed {
                                warmup = false;
                                debug!("Warming done (slot {})", block.slot);

                            }
                        }

                    } else {

                        if block.commitment_config == CommitmentConfig::confirmed() {
                            let prev_block = confirmed_blocks_by_slot.insert(block.slot, BlockDebugDetails {
                                blockhash: block.blockhash.clone(),
                                block: block.clone(),
                            });
                            assert!(prev_block.is_none(), "Must not see a confirmed block twice");
                        } else if block.commitment_config == CommitmentConfig::finalized() {
                            let finalized_block = &block;
                            let finalized_block_existed = finalized_blocks.insert(finalized_block.slot);
                            assert!(!finalized_block_existed, "Finalized block must have been seen before");
                            let prev_block = confirmed_blocks_by_slot.get(&block.slot);
                            match prev_block {
                                Some(prev_block) => {
                                    info!("Got finalized block {} with blockhash {} - prev confirmed was {}",
                                        finalized_block.slot, finalized_block.blockhash, prev_block.blockhash);
                                    // TODO is that correct?
                                    assert_eq!(finalized_block.blockhash, prev_block.blockhash, "Must see the same blockhash for confirmed and finalized block");

                                    debug!("confirmed: {:?}", to_string_without_transactions(&prev_block.block));
                                    debug!("finalized: {:?}", to_string_without_transactions(&finalized_block));

                                    assert_eq!(
                                        to_string_without_transactions(&prev_block.block).replace("commitment_config=confirmed", "commitment_config=IGNORE"),
                                        to_string_without_transactions(&finalized_block).replace("commitment_config=finalized", "commitment_config=IGNORE"),
                                        "block tostring mismatch"
                                    )

                                }
                                None => {
                                    // note at startup we might see some orphan finalized blocks before we see matching pairs of confirmed-finalized blocks
                                    panic!("Must see a confirmed block before it is finalized");
                                }
                            }
                            if prev_block.is_none() {
                                // note at startup we might see some finalized blocks before we see the confirmed blocks
                                warn!("Must see a confirmed block before it is finalized");
                            } else {
                            }
                        }
                    } // -- non-warmup

                    /// -- done

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
        "#,
        produced_block.leader_id,
        produced_block.blockhash,
        produced_block.block_height,
        produced_block.slot,
        produced_block.parent_slot,
        produced_block.block_time,
        produced_block.commitment_config.commitment,
        produced_block.previous_blockhash,
        // rewards
        // transactions
    )
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
