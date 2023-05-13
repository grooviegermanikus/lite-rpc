use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use solana_ledger::shred::Shred;
use solana_sdk::clock::Slot;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch::Sender;
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::Interval;



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
        shred_sender: Sender<Option<ShredSignal>>) -> JoinHandle<anyhow::Result<()>> {
        println!("start_service");
        // let tx_sender = self.tx_sender.clone();
        // let retry_after = self.retry_after;


        let join_handler =
            tokio::spawn(async move {
            // let mut reciever = reciever;
            let listen_socket = UdpSocket::bind("0.0.0.0:7999").await?;
            let mut buf = vec![0; 2048];
            let mut max_slot_seen = 0;
            loop {
                if let Ok((len, _peer)) = listen_socket.recv_from(&mut buf).await {

                    let shred = Shred::new_from_serialized_shred(buf[..len].to_vec());
                    match &shred {
                        Ok(shred) => {

                            // println!("slot: {:?}", shred.slot());
                            // println!("shred: {:?}", shred);

                            if shred.slot() > max_slot_seen {
                                max_slot_seen = shred.slot();
                                shred_sender.send(Some(ShredSignal::SlotSeen(max_slot_seen))).unwrap();
                            }

                        }
                        Err(e) => {
                            // println!("shred error: {:?}", e); // TODO investigate
                        }

                    }



                }

            } // -- loop
            Ok(())
        });

        join_handler
    }
}

