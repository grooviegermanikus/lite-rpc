use anyhow::bail;
use log::info;
use crate::proxy::QuicForwardProxy;
use crate::test_client::quic_test_client::QuicTestClient;
use crate::tls_config::SelfSignedTlsConfiguration;

mod proxy;
mod test_client;
mod quic_util;
mod tls_config;


#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let tls_configuration = SelfSignedTlsConfiguration::new_self_signed_localhost();

    let services = QuicForwardProxy::new(&tls_configuration).await?
        .start_services(
        );


    let test_client = QuicTestClient::new_with_endpoint(
        &tls_configuration
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
