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
use lite_rpc::shred_scanner::construct_entries::{shreds_for_slot_and_fecindex};
use lite_rpc::shred_scanner::types::ErasureSetId;


use lite_rpc::shred_scanner::vote_accounts_stakes::load_votestuff;

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

pub fn process_all_shreds(all_shreds: Vec<Shred>) {
    println!("processing {} shreds", all_shreds.len());

    let counts = all_shreds.iter().map(ErasureSetId::from).counts();
    println!("counts {:?}", counts);

    let CNT_DECODED = AtomicU64::new(0);

    // ErasureSetId
    for (esi, cnt) in counts.iter() {
        println!("{:?} count {} ...", esi, cnt);

        let only_my_slot = all_shreds.iter()
            .map(|s| {
                // println!("slot {}", s.slot());
                // println!("index {}", s.index());
                s
            })
            .filter(|s| ErasureSetId::from(*s) == *esi)
            .cloned()
            .collect_vec();




        // slot 197191944 0 count 83 ...
        // https://explorer.solana.com/block/197191944?cluster=testnet
        // successful transactions. 4547
        // process transactions: 5096
        // if *my_slot == 197191944 && *fec_index == 0 {
        println!("selected {} shreds for {:?}", only_my_slot.len(), esi);
        if false {
            for prefix_len in (1..50) {
                let mut first_n = only_my_slot.clone();
                first_n.truncate(prefix_len);
                shreds_for_slot_and_fecindex(first_n, &CNT_DECODED);
            }
        } else {
            shreds_for_slot_and_fecindex(only_my_slot, &CNT_DECODED);
        }


     } // -- for slots

    println!("could decode {}", CNT_DECODED.load(Relaxed));
}

