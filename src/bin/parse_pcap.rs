use std::collections::HashSet;
// Standalone binary to replay UDP traffic from a pcap file
use pcap_parser::*;
use pcap_parser::traits::PcapReaderIterator;
use std::fs::File;
use std::net::{SocketAddr, UdpSocket};
use std::ops::{Deref, Range, RangeTo};
use etherparse::IpNumber::Udp;
use etherparse::{SlicedPacket, TransportSlice};

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use solana_ledger::shred::{Error, ReedSolomonCache, Shred, ShredData};
use solana_sdk::clock::Slot;
use solana_sdk::hash::Hasher;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::Interval;



fn main() {


 let file = File::open("/Users/stefan/mango/projects/scan-shreds/shreds-big-sunday.pcap").unwrap();
    let mut num_blocks = 0;
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

                                num_blocks += 1;
                            }
                        }

                    }
                    PcapBlockOwned::LegacyHeader(_) => {}
                    PcapBlockOwned::NG(_) => {}
                }


                reader.consume(offset);
            },
            Err(PcapError::Eof) => {
                println!("EOF - read {num_blocks} blocks");
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
    let reed_solomon_cache = ReedSolomonCache::default();



    let counts = all_shreds.iter().map(|s| (s.slot(), s.fec_set_index()) ).counts();
    println!("counts {:?}", counts);

    // ErasureSetId
    for ((my_slot, fec_index),cnt) in counts.iter() {
        println!("slot {} {} count {} ...", my_slot, fec_index, cnt);

        let only_my_slot = all_shreds.iter()
            .map(|s| {
                // println!("slot {}", s.slot());
                // println!("index {}", s.index());
                s
            })
            .filter(|s| s.slot() == *my_slot && s.fec_set_index() == *fec_index)
            .cloned()
            .collect_vec();

        println!("selected {} shreds for slot {}", only_my_slot.len(), my_slot);

        //
        // recovery looks into first shred to get the slot which then is asserted to be the same for all other shreds
        // match solana_ledger::shred::recover(vec![only_my_slot.first().unwrap().clone()], &reed_solomon_cache) {

        // return `Error::TooFewShardsPresent` when there are not enough shards for reconstruction.
        let recovered = match solana_ledger::shred::recover(only_my_slot.clone(), &reed_solomon_cache) {
            Ok(recovered_shreds) => {
                println!("recovered {:?} shreds from {}", recovered_shreds.len(), only_my_slot.len());
                recovered_shreds
            }
            Err(err) => {
                println!("recover2 error {:?}", err);
                vec![]
            }
        };



        let mut collector: Vec<u8> = Vec::new();
        let mut indizes_seen: HashSet<u32> = HashSet::new();
        let mut last_in_slot_index = None;

        only_my_slot
            .iter()
            .chain(recovered.iter())
            .sorted_by_key(|s| s.index())
            .for_each(|s| {
                // let mut hasher = Hasher::default();
                // hasher.hash(&s.bytes_to_store());
                // println!("index {} hash {} (is_data={})", s.index(), hasher.result(), s.is_data());

                // println!("shred: {:?}", s);

                // TODO check vs s.last_in_slot()
                if s.data_complete() {
                    last_in_slot_index = Some(s.index());
                }

                match s {
                    Shred::ShredData(_) => {
                        collector.extend_from_slice(s.bytes_to_store());

                        indizes_seen.insert(s.index());

                        // let Shred::ShredData(daaaata) = s else { todo!(); };
                        // println!("daaaata {:?}", daaaata);
                    }
                    Shred::ShredCode(_) => {
                        collector.extend_from_slice(s.bytes_to_store());
                        indizes_seen.insert(s.index());
                    }
                }

            });

        println!("total data so far {}", collector.len());

        match last_in_slot_index {
            None => {
                println!("no complete packet seen for slot {}", my_slot);
            }
            Some(last) => {
                println!("last = {}", last);
                println!("indizes_seen(sorted) {:?}", indizes_seen.iter().sorted().collect_vec());
                let complete = check_if_complete(&indizes_seen, last);
                println!("completed {:?}", complete);
            }
        }

        // indizes_seen.sort();

        println!("indizes_seen {:?}", indizes_seen);


            // .map(|s| {
            //     let mut hasher = Hasher::default();
            //     hasher.hash(&s.bytes_to_store());
            //     println!("index {} hash {} (is_data={})", s.index(), hasher.result(), s.is_data());
            //     s
            // })
            // .dedup()
            // .filter_map(|shred| match shred {
            //     Shred::ShredCode(_) => None,
            //     Shred::ShredData(shred) => Some(shred),
            // })
            // .for_each(|shred| {
            //     println!("shred {:?}", shred);
            //     collector.append(shred);
            // });
        // Assert that the input data can be recovered from data shreds.





    } // -- for slots

    // TODO may need to check DATA_COMPLETE_SHRED

    // // The the max slot
    // let my_slot = winner.0.0;
    // let fec_index = winner.0.1;



    // println!("selected {} shreds for slot {}", only_my_slot.len(), my_slot);

}

fn check_if_complete(all_seen: &HashSet<u32>, last_index: u32) -> bool {

    if all_seen.len() != 1 + last_index as usize {
        return false;
    }

    return (0..=last_index).all(|i| all_seen.contains(&i));
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
                    println!("recovered {:?} shreds", recovered_shreds);
                }
                Err(err) => {
                    println!("recover1 error {:?}", err);
                }
            }
            // parse_sanitized_vote_transactio(&shred.payload);


        }
        Err(e) => {
            // println!("shred error: {:?}", e); // TODO investigate
        }

    }
}