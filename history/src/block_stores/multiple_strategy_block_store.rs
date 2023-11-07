// A mixed block store,
// Stores confirmed blocks in memory
// Finalized blocks in long term storage of your choice
// Fetches legacy blocks from faithful

use crate::block_stores::inmemory_block_store::InmemoryBlockStore;
use anyhow::{bail, Result};
use async_trait::async_trait;
use solana_lite_rpc_core::{
    commitment_utils::Commitment,
    structures::produced_block::ProducedBlock,
    traits::block_storage_interface::{BlockStorageImpl, BlockStorageInterface},
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::config::RpcBlockConfig;
use solana_sdk::{slot_history::Slot};
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use std::future::Future;
use log::debug;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};
use crate::block_stores::faithful_block_store::FaithfulBlockStore;

pub struct MultipleStrategyBlockStorage {
    inmemory_for_storage: InmemoryBlockStore, // for confirmed blocks
    persistent_block_storage: BlockStorageImpl, // for persistent block storage
    faithful_block_storage: Option<FaithfulBlockStore>, // to fetch legacy blocks from faithful
    last_confirmed_slot: Arc<AtomicU64>,
}

impl MultipleStrategyBlockStorage {
    pub fn new(
        persistent_block_storage: BlockStorageImpl,
        faithful_rpc_client: Option<Arc<RpcClient>>,
        number_of_slots_in_memory: usize,
    ) -> Self {
        Self {
            inmemory_for_storage: InmemoryBlockStore::new(number_of_slots_in_memory),
            persistent_block_storage,
            faithful_block_storage: faithful_rpc_client.map(|rpc| FaithfulBlockStore::new(rpc)),
            last_confirmed_slot: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn get_in_memory_block(&self, slot: Slot) -> anyhow::Result<ProducedBlock> {
        self.inmemory_for_storage
            .get(slot)
            .await
    }
}

#[async_trait]
impl BlockStorageInterface for MultipleStrategyBlockStorage {
    async fn save(&self, block: &ProducedBlock) -> Result<()> {
        let slot = block.slot;
        let commitment = block.commitment_level;
        match commitment {
            Commitment::Confirmed | Commitment::Processed => {
                self.inmemory_for_storage.save(block).await?;
            }
            Commitment::Finalized => {
                let block_in_mem = self.get_in_memory_block(block.slot).await;
                match block_in_mem {
                    Ok(block_in_mem) => {
                        // check if inmemory blockhash is same as finalized, update it if they are not
                        // we can have two machines with same identity publishing two different blocks on same slot
                        if block_in_mem.blockhash != block.blockhash {
                            self.inmemory_for_storage.save(block).await?;
                        }
                    }
                    Err(_) => self.inmemory_for_storage.save(block).await?,
                }
                self.persistent_block_storage.save(block).await?;
            }
        };
        if slot > self.last_confirmed_slot.load(Ordering::Relaxed) {
            self.last_confirmed_slot.store(slot, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn get(
        &self,
        slot: solana_sdk::slot_history::Slot,
    ) -> Result<ProducedBlock> {
        let last_confirmed_slot = self.last_confirmed_slot.load(Ordering::Relaxed);

        if slot > last_confirmed_slot {
            bail!(format!("Block {} not found (last_confirmed_slot={})", slot, last_confirmed_slot));
        }

        let range = self.inmemory_for_storage.get_slot_range().await;
        if range.contains(&slot) {
            let block = self.inmemory_for_storage.get(slot).await;
            match block {
                Ok(_) => {
                    debug!("Lookup for block {} successful in in-memory-storage", slot);
                    return block;
                }
                Err(_) => {
                    debug!("Block {} not found in in-memory-storage - continue", slot);
                }
            }
        }

        // TODO: Define what data is expected that is definetly not in persistant block storage like data after epoch - 1
        // check persistant block
        let persistent_block_range = self.persistent_block_storage.get_slot_range().await;
        match persistent_block_range.contains(&slot) {
            true => {
                debug!("Lookup for block {} successful in persistent block-storage", slot);
                return self.persistent_block_storage.get(slot).await;
            }
            false => {
                debug!("Block {} not found in persistent block-storage - continue", slot);
            }
        }

        if let Some(faithful_block_storage) = &self.faithful_block_storage {
            match faithful_block_storage.get_block(slot).await {
                Ok(block) => {
                    debug!("Lookup for block {} successful in faithful block-storage", slot);
                    return Ok(block);
                }
                Err(_) => {
                    debug!("Block {} not found in faithful storage - giving up", slot);
                    bail!(format!("Block {} not found in faithful", slot));
                }
            }
        } else {
            // no faithful available
            bail!(format!("Block {} not found - faithful not available", slot));
        }
    }

    async fn get_slot_range(&self) -> Range<Slot> {
        let in_memory = self.inmemory_for_storage.get_slot_range().await;
        // if faithful is available we assume that we have all the blocks
        if self.faithful_block_storage.is_some() {
            0..in_memory.end
        } else {
            let persistent_storage_range = self.persistent_block_storage.get_slot_range().await;
            persistent_storage_range.start..in_memory.end
        }
    }
}
