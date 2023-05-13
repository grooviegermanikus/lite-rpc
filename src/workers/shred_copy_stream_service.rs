use std::time::Duration;
use solana_ledger::shred::Shred;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time;
use tokio::time::Interval;

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
        // sender: UnboundedSender<u8>,
        // reciever: UnboundedReceiver<u8>,
    ) -> JoinHandle<anyhow::Result<()>> {
        println!("start_service");
        // let tx_sender = self.tx_sender.clone();
        // let retry_after = self.retry_after;


        tokio::spawn(async move {
            // let mut reciever = reciever;
            let listen_socket = UdpSocket::bind("0.0.0.0:7999").await?;
            let mut buf = vec![0; 2048];
            loop {
                if let Ok((len, peer)) = listen_socket.recv_from(&mut buf).await {



                    println!("got {len} bytes from {peer}");


                    let shred = Shred::new_from_serialized_shred(buf[..len].to_vec());
                    match shred {
                        Ok(shred) => {


                            println!("shred: {:?}", shred);
                            println!("slot: {:?}", shred.slot());
                        }
                        Err(e) => {
                            println!("shred error: {:?}", e);
                        }

                    }



                }

            } // -- loop
            Ok(())
        })
    }
}

