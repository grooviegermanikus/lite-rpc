use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use anyhow::bail;
use clap::Parser;
use dotenv::dotenv;
use log::info;
use crate::cli::{Args, get_identity_keypair};
use crate::proxy::QuicForwardProxy;
use crate::test_client::quic_test_client::QuicTestClient;
use crate::tls_config_provicer::SelfSignedTlsConfigProvider;

mod proxy;
mod tpu_quic_connection;
mod test_client;
mod quic_util;
mod tls_config_provicer;
mod cli;


#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let Args {
        identity_keypair,
    } = Args::parse();

    dotenv().ok();

    // TODO build args struct dedicyted to proxy
    let proxy_listener_addr = "127.0.0.1:11111".parse().unwrap();
    let tls_configuration = SelfSignedTlsConfigProvider::new_singleton_self_signed_localhost();
    let validator_identity = Arc::new(get_identity_keypair(&identity_keypair).await);


    let main_services = QuicForwardProxy::new(proxy_listener_addr, &tls_configuration, validator_identity)
        .await?
        .start_services();

    let proxy_addr = "127.0.0.1:11111".parse().unwrap();
    let test_client = QuicTestClient::new_with_endpoint(
        proxy_addr, &tls_configuration)
        .await?
        .start_services();


    let ctrl_c_signal = tokio::signal::ctrl_c();

    tokio::select! {
        res = main_services => {
            bail!("Services quit unexpectedly {res:?}");
        },
        res = test_client => {
            bail!("Test Client quit unexpectedly {res:?}");
        },
        _ = ctrl_c_signal => {
            info!("Received ctrl+c signal");

            Ok(())
        }
    }

}