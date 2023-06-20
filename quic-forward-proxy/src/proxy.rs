use std::time::Duration;
use anyhow::bail;
use log::info;
use solana_sdk::transaction::VersionedTransaction;
use tokio::net::ToSocketAddrs;
use solana_lite_rpc_core::AnyhowJoinHandle;

pub struct QuicForwardProxy {


}

impl QuicForwardProxy {
    pub async fn start_services(
        mut self,
    ) -> anyhow::Result<()> {
        let tx_sample_sender: AnyhowJoinHandle = tokio::spawn(async move {
            info!("Sample TX Server start");

            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                info!("Sending sample TX");
                // let tx = build_sample_tx();
                //
                // let raw_tx = bincode::serialize::<VersionedTransaction>(&tx).expect("failed to serialize tx");


                // transaction_service_sample.send_transaction(raw_tx).await.expect("should be enqueued");

                ticker.tick().await;
            }

            bail!("Sample TX server stopped");
        });

        tokio::select! {
            res = tx_sample_sender => {
                bail!("Sample TX server exited unexpectedly {res:?}");
            },
        }
    }
}

impl QuicForwardProxy {
    pub async fn new() -> anyhow::Result<Self> {

        todo!()
    }
}
