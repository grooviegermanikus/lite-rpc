use std::collections::{HashMap, HashSet};
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

use lite_rpc::shred_scanner::vote_accounts_stakes::load_votestuff;

use crate::CompletionState::{Complete, NumShredsYetUnknown, MissingDataByIndex, MissingDataBySize};

#[tokio::main]
async fn main() {

    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();
    let staking_info = load_votestuff().await.expect("could not load stakes");



    let path = "/Users/stefan/mango/projects/scan-shreds/shreds-big-sunday.pcap";
    let file = File::open(path).unwrap();
    let mut num_packets = 0;
    let mut reader = LegacyPcapReader::new(65536, file).expect("PcapNGReader");
    let mut all_shreds = Vec::with_capacity(10000);
    loop {
        match reader.next() {
            Ok((offset, block)) => {

                match block {
                    PcapBlockOwned::Legacy(leg_block) => {
                        let data = leg_block.data;
                        // println!("got new block {:?}", leg_block);

                        let eth = SlicedPacket::from_ethernet(&data).expect("from_ethernet");
                        if let Some(TransportSlice::Udp(ref udp)) = eth.transport {
                            if udp.destination_port() == 7999 {

                                // work o a single packet
                                use_packet(eth.clone());

                                match Shred::new_from_serialized_shred(eth.payload.to_vec()) {
                                    Ok(shred) => {
                                        all_shreds.push(shred);
                                    }
                                    Err(e) => {
                                        // ignore
                                    }
                                }

                                num_packets += 1;
                            }
                        }

                    }
                    PcapBlockOwned::LegacyHeader(_) => {}
                    PcapBlockOwned::NG(_) => {}
                }


                reader.consume(offset);
            },
            Err(PcapError::Eof) => {
                println!("EOF - read {num_packets} blocks");
                break;
            },
            Err(PcapError::Incomplete) => {
                reader.refill().unwrap();
            },
            Err(e) => panic!("error while reading: {:?}", e),
        }
    }


    process_all_shreds(all_shreds);


}


fn process_all_shreds(all_shreds: Vec<Shred>) {
    println!("processing {} shreds", all_shreds.len());

    // tuple (slot, fec_set_index) ErasureSetId: see shred.rs: Tuple which identifies erasure coding set that the shred belongs to

    let counts = all_shreds.iter().map(|s| (s.slot(), s.fec_set_index()) ).counts();
    println!("counts {:?}", counts);

    let CNT_DECODED = AtomicU64::new(0);

    // ErasureSetId
    for ((my_slot, fec_index), cnt) in counts.iter() {
        println!("slot {} fec_set_index {} count {} ...", my_slot, fec_index, cnt);

        let only_my_slot = all_shreds.iter()
            .map(|s| {
                // println!("slot {}", s.slot());
                // println!("index {}", s.index());
                s
            })
            .filter(|s| s.slot() == *my_slot && s.fec_set_index() == *fec_index)
            .cloned()
            .collect_vec();




        // slot 197191944 0 count 83 ...
        // https://explorer.solana.com/block/197191944?cluster=testnet
        // successful transactions. 4547
        // process transactions: 5096
        // if *my_slot == 197191944 && *fec_index == 0 {
        println!("selected {} shreds for slot {}", only_my_slot.len(), my_slot);
        if false {
            for prefix_len in (1..50) {
                let mut first_n = only_my_slot.clone();
                first_n.truncate(prefix_len);
                shreds_for_slot_and_fecindex(my_slot, first_n, &CNT_DECODED);
            }
        } else {
            shreds_for_slot_and_fecindex(my_slot, only_my_slot, &CNT_DECODED);
        }


     } // -- for slots

    println!("could decode {}", CNT_DECODED.load(Relaxed));
}

pub fn shreds_for_slot_and_fecindex(_my_slot: &Slot, only_my_slot: Vec<Shred>, CNT_DECODED: &AtomicU64) {
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


    let mut collector: HashMap<u32, &Shred> = HashMap::new();
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

            collector.insert(s.index(), s);
            indizes_seen.insert(s.index());

        });

    debug!("indizes_seen {:?}", indizes_seen);

    debug!("last = {last_index:?}");
    debug!("indizes_seen(sorted) {:?}", indizes_seen.iter().sorted().collect_vec());
    let complete = check_if_complete(&indizes_seen, last_index);
    debug!("completed status {:?}", complete);


    debug!("total data so far {}", collector.len());

    if let Complete(last_index) = complete {

        let attempt: Result<usize, Error> = Shredder::deshred(only_my_slot.as_slice()).map(|data| data.len());
        debug!("shredder recontructed size {:?}", attempt);

        // sort consecutive sequence by index
        let asdfsdfafsd = (0..=last_index).map(|i| {
            let shred = collector.get(&i).expect(format!("no shred for index {i}").as_str());
            shred.clone()
        }).cloned().collect_vec();


        let deshredded = Shredder::deshred(asdfsdfafsd.as_slice()).expect("Must deshred");

        info!("buffer size {}", deshredded.len());
        let decode_using_shredder = entries_from_blockdata_votes(deshredded);
        // debug!("decoding_shredder ... {:?}", decode_using_shredder);

        if let Ok(entries) = decode_using_shredder {
            info!("decodabled_shredder!");
            CNT_DECODED.fetch_add(1, Ordering::Relaxed);

            let votes = inspect_entries(entries);
            for vote in votes {
                println!("voted by {:?}", vote.voter)
            }
        }

    }


}

#[derive(Debug, Clone)]
struct Vote {
    pub voter: Pubkey,
    pub timestamp: Option<UnixTimestamp>,
    // signature of the bank's state at the last slot
    pub hash: Hash,
}

fn entries_from_blockdata_votes(data: Vec<u8>) -> bincode::Result<Vec<Entry>> {
    bincode::deserialize::<Vec<Entry>>(&data)
}

fn inspect_entries(entries: Vec<Entry>) -> Vec<Vote> {

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
                            hash: vote.hash,
                        }),
                        // new vote instruction - see  https://forum.solana.com/t/feature-compact-vote-state-1-14-17/174
                        VoteInstruction::CompactUpdateVoteState(vote) => Some(Vote {
                            //  1. `[SIGNER]` Vote authority
                            voter: account_keys[compiled_instruction.accounts[1] as usize],
                            timestamp: vote.timestamp,
                            hash: vote.hash,
                        }),
                        _ => None,
                    }

                })
                .for_each(|maybe_vote| if let Some(vote) = maybe_vote { collected_votes.push(vote) } );
        } // -- for tx

    } // -- for entries

    collected_votes
}

#[derive(Debug)]
enum CompletionState {
    Complete(u32), // OK, last_index
    NumShredsYetUnknown(u32),
    // check for size of index array failed
    MissingDataBySize(u32,u32), // (seen, last_index) - note: last_index is rangge-inclusive
    // check if all indices are there failed
    MissingDataByIndex

}
fn check_if_complete(all_seen: &HashSet<u32>, last_index: Option<u32>) -> CompletionState {

    match last_index {
        None => NumShredsYetUnknown(all_seen.len() as u32),
        Some(last_index) => {

            if all_seen.len() != 1 + last_index as usize {
                return MissingDataBySize(all_seen.len() as u32, last_index)
            }

            if !(0..=last_index).all(|i| all_seen.contains(&i)) {
                return MissingDataByIndex; // TODO provide missing indices
            }

            return Complete(last_index);
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