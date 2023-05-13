use std::time::Duration;
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
            let mut buf = vec![0; 1024];
            loop {
                if let Ok((len, peer)) = listen_socket.recv_from(&mut buf).await {

                    println!("got {len} bytes from {peer}");
                }

            } // -- loop
            Ok(())
        })
    }
}
