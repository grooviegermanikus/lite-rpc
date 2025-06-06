use crate::cli::Args;
use crate::proxy::QuicForwardProxy;
use anyhow::bail;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::EncodableKey;
use crate::validator_identity::ValidatorIdentity;

pub mod cli;
mod inbound;
mod outbound;
pub mod proxy;
pub mod proxy_request_format;
pub mod quic_util;
mod quinn_auto_reconnect;
mod shared;
mod util;
mod validator_identity;
mod solana_tls_config;
mod skip_client_verification;
mod skip_server_verification;

#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let Args {
        identity_keypair,
        proxy_listen_addr,
    } = Args::parse();
    dotenv().ok();

    let proxy_listener_addr = proxy_listen_addr.parse().unwrap();

    let validator_identity = ValidatorIdentity::new(
        Some(identity_keypair).filter(|s| !s.is_empty())
            .map(|key_file| Keypair::read_from_file(key_file).unwrap())
    );

    let main_services = QuicForwardProxy::new(proxy_listener_addr, validator_identity)
        .await?
        .start_services();

    let ctrl_c_signal = tokio::signal::ctrl_c();

    tokio::select! {
        res = main_services => {
            bail!("Services quit unexpectedly {res:?}");
        },
        // res = test_client => {
        //     bail!("Test Client quit unexpectedly {res:?}");
        // },
        _ = ctrl_c_signal => {
            info!("Received ctrl+c signal");

            Ok(())
        }
    }
}
