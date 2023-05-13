use std::time::Duration;

use anyhow::bail;
use clap::Parser;
use dotenv::dotenv;
use lite_rpc::{bridge::LiteBridge, cli::Args};
use log::info;
use solana_sdk::signature::Keypair;
use std::env;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::select;
use lite_rpc::workers::ShredCopyStreamService;

async fn get_identity_keypair(identity_from_cli: &String) -> Keypair {
    if let Ok(identity_env_var) = env::var("IDENTITY") {
        if let Ok(identity_bytes) = serde_json::from_str::<Vec<u8>>(identity_env_var.as_str()) {
            Keypair::from_bytes(identity_bytes.as_slice()).unwrap()
        } else {
            // must be a file
            let identity_file = tokio::fs::read_to_string(identity_env_var.as_str())
                .await
                .expect("Cannot find the identity file provided");
            let identity_bytes: Vec<u8> = serde_json::from_str(&identity_file).unwrap();
            Keypair::from_bytes(identity_bytes.as_slice()).unwrap()
        }
    } else if identity_from_cli.is_empty() {
        Keypair::new()
    } else {
        let identity_file = tokio::fs::read_to_string(identity_from_cli.as_str())
            .await
            .expect("Cannot find the identity file provided");
        let identity_bytes: Vec<u8> = serde_json::from_str(&identity_file).unwrap();
        Keypair::from_bytes(identity_bytes.as_slice()).unwrap()
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() {

    let shred_copy_stream_service = ShredCopyStreamService::new();

    // TODO guess broadcast is wrong type of channel
    let (shred_sender, mut shred_receiver) = tokio::sync::watch::channel(None);
    let max_slot_seen = Arc::new(AtomicU64::new(0));

    // shred_copy_stream_service.start_service(shred_sender, max_slot_seen).await?

    // let join_handle = shred_copy_stream_service.start_service(shred_sender, max_slot_seen);

    let mut join_handle = shred_copy_stream_service.start_service(shred_sender);

    loop {
        select! {
            val = shred_receiver.changed() => {
                if let Some(shred) = *shred_receiver.borrow() {
                    println!("shred changed to {:?}", shred);
                }
            }
            _ = &mut join_handle => {
                println!("join_handle exited");
                break;
            }
        }
    }

    join_handle.await.unwrap();


}



#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn _main_zzzz() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let Args {
        rpc_addr,
        ws_addr,
        lite_rpc_ws_addr,
        lite_rpc_http_addr,
        clean_interval_ms,
        fanout_size,
        enable_postgres,
        prometheus_addr,
        identity_keypair,
        maximum_retries_per_tx,
        transaction_retry_after_secs,
    } = Args::parse();

    dotenv().ok();

    let identity = get_identity_keypair(&identity_keypair).await;

    let clean_interval_ms = Duration::from_millis(clean_interval_ms);

    let retry_after = Duration::from_secs(transaction_retry_after_secs);
    let light_bridge = LiteBridge::new(
        rpc_addr,
        ws_addr,
        fanout_size,
        identity,
        retry_after,
        maximum_retries_per_tx,
    )
    .await?;

    let services = light_bridge
        .start_services(
            lite_rpc_http_addr,
            lite_rpc_ws_addr,
            clean_interval_ms,
            enable_postgres,
            prometheus_addr,
        )
        .await?;

    let services = futures::future::try_join_all(services);

    let ctrl_c_signal = tokio::signal::ctrl_c();

    tokio::select! {
        _ = services => {
            bail!("Services quit unexpectedly");
        }
        _ = ctrl_c_signal => {
            info!("Received ctrl+c signal");
            Ok(())
        }
    }
}
