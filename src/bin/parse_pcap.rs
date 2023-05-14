use std::collections::{HashMap, HashSet};
// Standalone binary to replay UDP traffic from a pcap file
use pcap_parser::*;
use pcap_parser::traits::PcapReaderIterator;
use std::fs::File;
use std::net::{SocketAddr, UdpSocket};
use std::ops::{Deref, Range, RangeTo};
use etherparse::IpNumber::Udp;
use etherparse::{SlicedPacket, TransportSlice};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use env_logger::Env;
use itertools::Itertools;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, ReedSolomonCache, Shred, ShredData, Shredder};
use solana_sdk::clock::Slot;
use solana_sdk::hash::Hasher;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::Interval;
use crate::CompletionState::{Complete, DataCompleteNotYetSeen, MissingDataByIndex, MissingDataBySize};


fn main() {

    env_logger::Builder::from_env(Env::default().default_filter_or("debug")).init();


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


    let counts = all_shreds.iter().map(|s| (s.slot(), s.fec_set_index()) ).counts();
    println!("counts {:?}", counts);

    // ErasureSetId
    for ((my_slot, fec_index),cnt) in counts.iter() {
        // println!("slot {} {} count {} ...", my_slot, fec_index, cnt);

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
                shreds_for_slot_and_fecindex(my_slot, first_n);
            }
        } else {
            shreds_for_slot_and_fecindex(my_slot, only_my_slot);
        }


     } // -- for slots

    println!("could decode {}", CNT_DECODED.load(Ordering::Relaxed));
}

const CNT_DECODED: AtomicU64 = AtomicU64::new(0);

pub fn shreds_for_slot_and_fecindex(_my_slot: &Slot, only_my_slot: Vec<Shred>) {
    let reed_solomon_cache = ReedSolomonCache::default();


    // recovery looks into first shred to get the slot which then is asserted to be the same for all other shreds
    // match solana_ledger::shred::recover(vec![only_my_slot.first().unwrap().clone()], &reed_solomon_cache) {

    // return `Error::TooFewShardsPresent` when there are not enough shards for reconstruction.
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


    let mut collector: HashMap<u32, &Shred> = HashMap::new();
    // TDOO redundant
    let mut indizes_seen: HashSet<u32> = HashSet::new();
    let mut last_index = None;

    only_my_slot
        .iter()
        .chain(recovered.iter())
        .for_each(|s| {
            // let mut hasher = Hasher::default();
            // hasher.hash(&s.bytes_to_store());
            // println!("index {} hash {} (is_data={})", s.index(), hasher.result(), s.is_data());

            // println!("shred: {:?}", s);

            // TODO check vs s.last_in_slot()
            // see deshred for logic
            if s.data_complete() || s.last_in_slot() {
                last_index = Some(s.index());
            }

            match s {
                Shred::ShredData(_) => {
                    // FIXME bytes_to_store is maybe wrong

                    collector.insert(s.index(), s);
                    indizes_seen.insert(s.index());

                    // let Shred::ShredData(daaaata) = s else { todo!(); };
                    // println!("daaaata {:?}", daaaata);
                }
                Shred::ShredCode(_) => {
                    // TODO check if we should do that?
                    // collector.insert(s.index(), s);
                    // indizes_seen.insert(s.index());
                }
            }
        });

    debug!("indizes_seen {:?}", indizes_seen);

    debug!("last = {last_index:?}");
    debug!("indizes_seen(sorted) {:?}", indizes_seen.iter().sorted().collect_vec());
    let complete = check_if_complete(&indizes_seen, last_index);
    debug!("completed status {:?}", complete);

    let attempt: Result<usize, Error> = Shredder::deshred(only_my_slot.as_slice()).map(|data| data.len());
    debug!("shredder recontructed size {:?}", attempt);

    debug!("total data so far {}", collector.len());

    if let Complete(last_index) = complete {

        let mut buffer = Vec::new();

        (0..=last_index).map(|i| {
            let shred = collector.get(&i).expect(format!("no shred for index {i}").as_str());
            *shred
        }).map(|s| s.data().unwrap())
        .for_each(|data| {
            // let shred_bufs: Vec<_> = shreds.iter().map(Shred::payload).cloned().collect();
            buffer.extend_from_slice(&data);
        });

        info!("buffer size {}", buffer.len());
        let decode = entries_from_blockdata(buffer);
        info!("decoding ... {:?}", decode);
        if decode.is_ok() {
            CNT_DECODED.fetch_add(1, Ordering::Relaxed);
        }
    }


}

fn entries_from_blockdata(data: Vec<u8>) -> bincode::Result<Vec<Entry>> {
    bincode::deserialize::<Vec<Entry>>(&data)
}

#[derive(Debug)]
enum CompletionState {
    Complete(u32), // OK, last_index
    DataCompleteNotYetSeen(u32),
    // check for size of index array failed
    MissingDataBySize(u32,u32), // (seen, last_index) - note: last_index is rangge-inclusive
    // check if all indices are there failed
    MissingDataByIndex

}
fn check_if_complete(all_seen: &HashSet<u32>, last_index: Option<u32>) -> CompletionState {

    match last_index {
        None => DataCompleteNotYetSeen(all_seen.len() as u32),
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