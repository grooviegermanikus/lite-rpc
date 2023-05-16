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
use solana_rpc_client_api::response::SlotUpdate::Completed;
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
use lite_rpc::shred_scanner::construct_entries::{extract_votes_from_entries, extract_entries_from_complete_slots, shreds_for_slot_and_fecindex, Vote, CompletionState};
use lite_rpc::shred_scanner::construct_entries::CompletionState::Complete;
use lite_rpc::shred_scanner::types::ErasureSetId;


use lite_rpc::shred_scanner::vote_accounts_stakes::{load_votestuff, StakingInfo};

#[tokio::main]
async fn main() {

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let staking_info = load_votestuff().await.expect("could not load stakes");


    // tcpdump -i lo udp port 7999
    // let path = "/Users/stefan/mango/projects/scan-shreds/shreds-big-sunday.pcap";
    // let path = "/Users/stefan/mango/projects/scan-shreds/shreds-big-tuesday.pcap";

    // tcpdump -i enp33s0f0 udp portrange 8000-8800 and inbound
    let path = "/Users/stefan/mango/projects/scan-shreds/shreds-inbound-tuesday.pcap";

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
                            if udp.destination_port() == 7999 || (udp.destination_port() == 8000 && udp.destination_port() <= 8800) {

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


    let mut latest_vote_per_voter = HashMap::new();
    process_all_shreds(&all_shreds, &staking_info, &mut latest_vote_per_voter);


}

pub fn process_all_shreds(all_shreds: &Vec<Shred>, staking_info: &StakingInfo, latest_vote_per_voter: &mut HashMap<Pubkey, Vote>) {
    println!("processing {} shreds", all_shreds.len());

    let reed_solomon_cache = ReedSolomonCache::default();

    // let counts = all_shreds.iter().map(ErasureSetId::from).counts();

    let CNT_DECODED = AtomicU64::new(0);


    let count_total_coding = all_shreds.iter()
        .filter(|s| s.is_code())
        .count();
    println!("count_total_coding {:?}", count_total_coding);

    // ErasureSetId
    for current_slot in all_shreds.iter().map(|s| s.slot()).unique().sorted() {
    // for current_slot in vec![197444561] {

        let mut entries_per_slot = Vec::new();

        let shreds_per_slot = all_shreds.iter().filter(|s| s.slot() == current_slot).count();
        println!("selected {} shreds for slot {}", shreds_per_slot, current_slot);

        let esi_per_slot = all_shreds.iter().filter(|s| s.slot() == current_slot).map(ErasureSetId::from).unique().collect_vec();
        for esi in esi_per_slot {
            let only_my_slot = all_shreds.iter()
                // .filter(|s| ErasureSetId::from(*s).slot() == esi.slot())
                .filter(|s| ErasureSetId::from(*s) == esi)
                .cloned()
                .collect_vec();
            // println!("selected {} shreds for {:?}", only_my_slot.len(), esi);

            let only_my_slot_count = only_my_slot.iter()
                .filter(|s| s.is_code())
                .count();
            // println!("only_my_slot1 {:?}", only_my_slot_count);

            let mut completed_state = shreds_for_slot_and_fecindex(&only_my_slot, &CNT_DECODED, &reed_solomon_cache);

            match completed_state {
                Complete(last_index, collector) => {
                    let entries = extract_entries_from_complete_slots(collector, last_index);
                    // println!("got {} entries", entries.len());
                    entries_per_slot.extend(entries);
                }
                _ => {
                    println!("completed state (error): {:?}", completed_state);
                }
            }
        } // -- for all fec_sets of this slot

        println!("got {} entries for slot {} (all fec indices)", entries_per_slot.len(), current_slot);

        // let only_my_slot = all_shreds.iter()
        //     .filter(|s| ErasureSetId::from(*s) == *esi)
        //     .cloned()
        //     .collect_vec();


        let votes = extract_votes_from_entries(entries_per_slot);
        for vote in &votes {
            // println!("vote: {:?}", vote);
            let last_vote = latest_vote_per_voter.get_mut(&vote.voter);
            if let Some(last_vote) = last_vote {
                if last_vote.slot < vote.slot {
                    *last_vote = vote.clone();
                }
            } else {
                latest_vote_per_voter.insert(vote.voter, vote.clone());
            }
        }

        // how many voters put their stake on the rooted slot?
        let rooted_slot = calc_rooted_slot(&votes, latest_vote_per_voter, staking_info);
        println!("rooted slot: {:?}", rooted_slot);

        //
        //
        // let completed_state = shreds_for_slot_and_fecindex(&only_my_slot, &CNT_DECODED);
        // // println!("completed_state {:?}", completed_state);
        // if let Complete(last_index, collector) = completed_state {
        //     let entries = extract_entries_from_complete_slots(collector, last_index);
        //     println!("got {} entries", entries.len());
        //     let votes = extract_votes_from_entries(entries);
        //     for vote in &votes {
        //         let last_vote = latest_vote_per_voter.get_mut(&vote.voter);
        //         if let Some(last_vote) = last_vote {
        //             if last_vote.slot < vote.slot {
        //                 *last_vote = vote.clone();
        //             }
        //         } else {
        //             latest_vote_per_voter.insert(vote.voter, vote.clone());
        //         }
        //         // println!("vote: {:?}", vote);
        //     }
        //     let rooted_slot = calc_rooted_slot(&votes, latest_vote_per_voter, staking_info);
        //     println!("rooted slot: {:?}", rooted_slot);
        // }
        //

     } // -- for slots

    // println!("could decode {}", CNT_DECODED.load(Relaxed));
}

fn calc_rooted_slot(_votes: &Vec<Vote>, latest_vote_per_voter: &HashMap<Pubkey, Vote>, staking_info: &StakingInfo) -> Option<(Slot, f64)> {

    // 1. voter -> stake
    // 2. voter -> max root slot


    // let stakes_per_root_slot = votes.iter().map(|vote| (vote.root_slot, staking_info.get_stake(&vote.voter))).into_grouping_map().sum();
    let stakes_per_root_slot = latest_vote_per_voter.iter().map(|(voter, vote)| (vote.root_slot, staking_info.get_stake(voter))).into_grouping_map().sum();
    let total_stake = staking_info.get_total_stake() as f64;
    let majority_stake = total_stake * 0.067;

    // println!("stakes_per_root_slot {:?}", stakes_per_root_slot);
    stakes_per_root_slot.iter().sorted_by_key(|(slot, _)| *slot)
        .rfind(|(slot, stake)| **stake as f64 > majority_stake)
        .map(|(slot, stake)| (*slot, *stake as f64 / total_stake))


}

