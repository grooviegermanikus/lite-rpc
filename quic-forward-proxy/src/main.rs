use crate::cli::{get_identity_keypair, Args};
use crate::proxy::QuicForwardProxy;
use crate::tls_self_signed_pair_generator::SelfSignedTlsConfigProvider;
use anyhow::bail;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use std::sync::Arc;

use crate::validator_identity::ValidatorIdentity;

pub mod cli;
mod inbound;
mod outbound;
pub mod proxy;
pub mod proxy_request_format;
pub mod quic_util;
mod quinn_auto_reconnect;
mod shared;
pub mod test_client;
pub mod tls_config_provider_client;
pub mod tls_config_provider_server;
pub mod tls_self_signed_pair_generator;
mod util;
mod validator_identity;


#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let Args {
        identity_keypair,
        proxy_listen_addr: proxy_listen_addr,
        // TEMP used to get blockhash
        rpc_url,
    } = Args::parse();

    dotenv().ok();

    // TODO build args struct dedicated to proxy
    let proxy_listener_addr = proxy_listen_addr.parse().unwrap();
    let _tls_configuration = SelfSignedTlsConfigProvider::new_singleton_self_signed_localhost();
    let validator_identity = ValidatorIdentity::new(get_identity_keypair(&identity_keypair).await);

    let tls_config = Arc::new(SelfSignedTlsConfigProvider::new_singleton_self_signed_localhost());
    let quic_proxy = QuicForwardProxy::new(proxy_listener_addr, tls_config, validator_identity)
        .await?;

    if true {
        quic_proxy.send_example_tx(rpc_url).await;
        return Ok(());
    }

    let main_services = quic_proxy.start_services();

    // let proxy_addr = "127.0.0.1:11111".parse().unwrap();
    // let test_client = QuicTestClient::new_with_endpoint(
    //     proxy_addr, &tls_configuration)
    //     .await?
    //     .start_services();

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
