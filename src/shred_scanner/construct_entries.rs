use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
// Standalone binary to replay UDP traffic from a pcap file
use pcap_parser::*;
use pcap_parser::traits::PcapReaderIterator;
use std::fs::File;
use std::iter::FlatMap;
use std::net::{SocketAddr, UdpSocket};
use std::ops::{Deref, Range, RangeTo};
use std::slice::Iter;
use etherparse::IpNumber::Udp;
use etherparse::{SlicedPacket, TransportSlice};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};
use env_logger::Env;
use itertools::Itertools;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, ReedSolomonCache, Shred, ShredData, Shredder};
use solana_sdk::clock::{Slot, UnixTimestamp};
use solana_sdk::hash::{Hash, Hasher};
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{SanitizedVersionedMessage, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::{SanitizedVersionedTransaction, VersionedTransaction};
use solana_sdk::vote::instruction::VoteInstruction;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time;

use CompletionState::*;


pub fn shreds_for_slot_and_fecindex(only_my_slot: &Vec<Shred>, CNT_DECODED: &AtomicU64) -> CompletionState {
    // TODO make this more global (wee window_service - the cache is a singleton)
    let reed_solomon_cache = ReedSolomonCache::default();


    // recovery looks into first shred to get the slot which then is asserted to be the same for all other shreds
    // match solana_ledger::shred::recover(vec![only_my_slot.first().unwrap().clone()], &reed_solomon_cache) {

    // recover is also used from blockstore.insert_shreds_handle_duplicate
    let recovered = match solana_ledger::shred::recover(only_my_slot.clone(), &reed_solomon_cache) {
        Ok(recovered_shreds) => {
            debug!("recovered {:?} shreds from {}", recovered_shreds.len(), only_my_slot.len());
            recovered_shreds
        }
        Err(err) => {
            debug!("recover2 error {:?}", err);
            vec![]
        }
    };

    assert!(recovered.iter().all(|shred| shred.is_data()),
            "recovery produces only data shreds");


    let mut collector: HashMap<u32, Shred> = HashMap::new();
    // TDOO redundant
    let mut indizes_seen: HashSet<u32> = HashSet::new();
    let mut last_index: Option<u32> = None;

    only_my_slot
        .iter()
        .filter(|shred| shred.is_data())
        .chain(recovered.iter())
        .for_each(|s| {

            if let Ok(num_data_shreds) = s.num_data_shreds() {
                // u16 -> u32 .. should be okey, see shred_code.rs "u32::from"
                last_index = Some(num_data_shreds as u32);
            }

            // see deshred for logic
            if s.data_complete() || s.last_in_slot() {
                last_index = Some(s.index());
            }

            assert!(s.is_data());

            collector.insert(s.index(), s.clone());
            indizes_seen.insert(s.index());

        });

    debug!("indizes_seen {:?}", indizes_seen);

    debug!("last = {last_index:?}");
    debug!("indizes_seen(sorted) {:?}", indizes_seen.iter().sorted().collect_vec());
    let complete = check_if_complete(&indizes_seen, collector, last_index);
    debug!("completed status {:?}", complete);


    // debug!("total data so {}", collector.len());

    complete

}

pub fn extract_entries_from_complete_slots(collector: HashMap<u32, Shred>, last_index: u32) -> Vec<Entry> {

    // let attempt: Result<usize, Error> = Shredder::deshred(only_my_slot.as_slice()).map(|data| data.len());
    // println!("shredder recontructed size {:?}", attempt);

    // sort consecutive sequence by index
    let sorted_shreds = (0..=last_index).map(|i| {
        let shred = collector.get(&i).expect(format!("no shred for index {i}").as_str());
        shred.clone()
    }).collect_vec();

    println!("sorted_shreds {:?}", sorted_shreds.len());
    Shredder::deshred(sorted_shreds.as_slice())
        .map(|deshredded| {
            entries_from_blockdata_votes(deshredded).expect("must decode")
        }).unwrap_or(vec![])
}

#[derive(Debug, Clone)]
pub struct Vote {
    pub voter: Pubkey,
    pub timestamp: Option<UnixTimestamp>,
    // signature of the bank's state at the last slot
    pub block_hash: Hash,
    // TODO add timestamp
}

fn entries_from_blockdata_votes(data: Vec<u8>) -> bincode::Result<Vec<Entry>> {
    bincode::deserialize::<Vec<Entry>>(&data)
}

pub fn extract_votes_from_entries(entries: Vec<Entry>) -> Vec<Vote> {

    let mut collected_votes = Vec::new();

    for entry in entries {
        for tx in entry.transactions {
            let transaction = SanitizedVersionedTransaction::try_from(tx).unwrap();
            let msg = transaction.get_message();

            let account_keys = msg.message.static_account_keys();

            msg.message.instructions().iter()
                .filter(|compiled_instruction| account_keys[compiled_instruction.program_id_index as usize] == solana_vote_program::id())
                .map(|compiled_instruction| {
                    let vote_instruction = bincode::deserialize::<VoteInstruction>(compiled_instruction.data.as_slice()).unwrap();

                    match vote_instruction {
                        VoteInstruction::Vote(vote) => Some(Vote {
                            //  1. `[SIGNER]` Vote authority
                            voter: account_keys[compiled_instruction.accounts[1] as usize],
                            timestamp: vote.timestamp,
                            block_hash: vote.hash,
                        }),
                        // new vote instruction - see  https://forum.solana.com/t/feature-compact-vote-state-1-14-17/174
                        VoteInstruction::CompactUpdateVoteState(vote) => Some(Vote {
                            //  1. `[SIGNER]` Vote authority
                            voter: account_keys[compiled_instruction.accounts[1] as usize],
                            timestamp: vote.timestamp,
                            block_hash: vote.hash,
                        }),
                        VoteInstruction::CompactUpdateVoteStateSwitch(vote, _hash) => Some(Vote {
                            //  1. `[SIGNER]` Vote authority
                            voter: account_keys[compiled_instruction.accounts[1] as usize],
                            timestamp: vote.timestamp,
                            block_hash: vote.hash,
                        }),
                        _ => None,
                    }

                })
                .for_each(|maybe_vote| if let Some(vote) = maybe_vote { collected_votes.push(vote) } );
        } // -- for tx

    } // -- for entries

    println!("collected_votes {:?}", collected_votes.len());

    collected_votes
}

#[derive(Debug)]
pub enum CompletionState {
    Complete(u32, HashMap<u32, Shred>), // OK, last_index
    NumShredsYetUnknown(u32),
    // check for size of index array failed
    MissingDataBySize(u32,u32), // (seen, last_index) - note: last_index is rangge-inclusive
    // check if all indices are there failed
    MissingDataByIndex

}
fn check_if_complete(all_seen: &HashSet<u32>, collector: HashMap<u32, Shred>, last_index: Option<u32>) -> CompletionState {

    match last_index {
        None => NumShredsYetUnknown(all_seen.len() as u32),
        Some(last_index) => {

            if all_seen.len() != 1 + last_index as usize {
                return MissingDataBySize(all_seen.len() as u32, last_index)
            }

            if !(0..=last_index).all(|i| all_seen.contains(&i)) {
                return MissingDataByIndex; // TODO provide missing indices
            }

            return Complete(last_index, collector);
        }
    }
}


fn use_packet(eth: SlicedPacket) {
    if true {
        return;
    }

    let reed_solomon_cache = ReedSolomonCache::default();

    match Shred::new_from_serialized_shred(eth.payload.to_vec()) {
        Ok(shred) => {

            // get_shred_variant(&shred.payload).unwrap();
            // println!("got shred {:?}", shred);
            match solana_ledger::shred::recover(vec![], &reed_solomon_cache) {
                Ok(recovered_shreds) => {
                    debug!("recovered {:?} shreds", recovered_shreds);
                }
                Err(err) => {
                    warn!("recover1 error {:?}", err);
                }
            }
            // parse_sanitized_vote_transactio(&shred.payload);


        }
        Err(e) => {
            // println!("shred error: {:?}", e); // TODO investigate
        }

    }
}
