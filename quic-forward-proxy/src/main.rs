use anyhow::bail;
use log::info;
use lite_rpc_quic_forward_proxy::tls_config::SelfSignedTlsConfiguration;
use crate::proxy::QuicForwardProxy;
use crate::test_client::quic_test_client::QuicTestClient;

mod proxy;
mod test_client;
mod quic_util;
mod tls_config;


#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let services = QuicForwardProxy::new().await?
        .start_services(
        );

    let test_client = QuicTestClient::new_with_endpoint(
        &SelfSignedTlsConfiguration::new_self_signed_localhost()
    ).await?
        .start_services(
        );

    let ctrl_c_signal = tokio::signal::ctrl_c();

    tokio::select! {
        res = services => {
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
