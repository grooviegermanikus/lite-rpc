// Standalone binary to replay UDP traffic from a pcap file
use pcap_parser::*;
use pcap_parser::traits::PcapReaderIterator;
use std::fs::File;
use std::net::{SocketAddr, UdpSocket};
use etherparse::IpNumber::Udp;
use etherparse::{SlicedPacket, TransportSlice};

fn main() {


    let sender_copy_udp = UdpSocket::bind("0.0.0.0:0".parse::<SocketAddr>().expect("parse")).expect("bind");
    let target: SocketAddr = "127.0.0.1:7999".parse().expect("parse");

    loop {
     let file = File::open("/Users/stefan/mango/projects/scan-shreds/shreds.pcap").unwrap();
        let mut num_blocks = 0;
        let mut reader = LegacyPcapReader::new(65536, file).expect("PcapNGReader");
        loop {
            match reader.next() {
                Ok((offset, block)) => {


                    match block {
                        PcapBlockOwned::Legacy(leg_block) => {
                            let data = leg_block.data;
                            // println!("got new block {:?}", leg_block);

                            let eth = SlicedPacket::from_ethernet(&data).expect("from_ethernet");
                            if let Some(TransportSlice::Udp(udp)) = eth.transport {
                                if udp.destination_port() == target.port() {
                                    sender_copy_udp.send_to(eth.payload, target).expect("send_to");
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
                    println!("EOF - restart {num_blocks}");
                    break;
                },
                Err(PcapError::Incomplete) => {
                    reader.refill().unwrap();
                },
                Err(e) => panic!("error while reading: {:?}", e),
            }
        }
    }


}