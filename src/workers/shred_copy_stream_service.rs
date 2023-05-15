use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use pcap_parser::parse_pcap;
use solana_ledger::shred::parse_pcap::shreds_for_slot_and_fecindex;
use solana_ledger::shred::Shred;
use solana_sdk::clock::Slot;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::Interval;
use crate::block_store::BlockStore;
use crate::shred_scanner::types::ErasureSetId;
use crate::workers::tpu_utils::tpu_service::{IdentityStakes, VoteStakingInfo};


// test using /Users/stefan/mango/code/lite-rpc/target/debug/replay_udp_traffic

#[derive(Clone, Copy, Debug)]
pub enum ShredSignal {
    /// highes slot number seen so far
    SlotSeen(Slot),
}

#[derive(Clone)]
pub struct ShredCopyStreamService {
}


/// subscribe to UDP stream of shres produce from ShredFetchStage
impl ShredCopyStreamService {
    pub fn new() -> Self {
        Self {
        }
    }

    pub fn start_service(
        &self,
        shred_sender: Sender<Option<ShredSignal>>,
        tpu_estimated_slot: Arc<AtomicU64>,
        block_store: BlockStore,
        vote_accounts_stakes: Arc<RwLock<VoteStakingInfo>>)
    -> JoinHandle<anyhow::Result<()>> {
        println!("start shred service");
        let join_handler =
            tokio::spawn(async move {
            let listen_socket = UdpSocket::bind("0.0.0.0:7999").await?;
            let mut buf = vec![0; 2048];
            let mut max_slot_seen = 0;
            let mut debounce_log = 0;


            // very naive shred collection - potential improvements:
            // - ignore shreds older than X
            // - remove data for completed block
            // - dedup shreds

            let mut shred_buffer_map: HashMap<ErasureSetId, Vec<Shred>> = HashMap::new();

            loop {
                match listen_socket.recv_from(&mut buf).await {
                    Ok((len, _peer)) => {

                        // TODO optimizze
                        match Shred::new_from_serialized_shred(buf[..len].to_vec()) {
                            Ok(shred) => {

                                println!("slotttt {}", shred.slot());
                                if shred.slot() != 197191954 {
                                    continue;
                                }

                                if shred.slot() > max_slot_seen {
                                    max_slot_seen = shred.slot();
                                    assert_ne!(max_slot_seen, 0, "never send the initial value");
                                    // shred_sender.send(Some(ShredSignal::SlotSeen(max_slot_seen))).unwrap();

                                    println!("slot from shred vs estimated slot {:?} vs {:?}",
                                             tpu_estimated_slot.load(Relaxed),
                                             shred.slot());
                                }

                                let esi = ErasureSetId::from(&shred);

                                let shreds = shred_buffer_map.entry(esi)
                                    .or_insert_with(|| vec![]);
                                shreds.push(shred);

                                debounce_log += 1;
                                if debounce_log % 1000 == 0 {
                                    println!("shred_buffer_map len: {}", shred_buffer_map.len());
                                    Self::process_all(&shred_buffer_map);
                                }



                                // play_with_process_all_shreds(shred, block_store.clone(), vote_accounts_stakes.clone()).await;


                                // block_store.get_block_info(shred.slot()


                            }
                            Err(e) => {
                                println!("shred error: {:?}", e); // TODO investigate
                            }

                        }

                    }
                    Err(_) => {
                        println!("sock recv error");
                    }
                }

            } // -- loop
            Ok(())
        });

        join_handler
    }

    fn process_all(mut shred_buffer_map: &HashMap<ErasureSetId, Vec<Shred>>) {
        for (esi, shreds) in shred_buffer_map {
            shreds_for_slot_and_fecindex(shreds)
        }
    }
}


