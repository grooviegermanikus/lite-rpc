use anyhow::bail;
use log::info;
use crate::proxy::QuicForwardProxy;

mod proxy;

#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let services = QuicForwardProxy::new().await?
        .start_services(
        );

    let ctrl_c_signal = tokio::signal::ctrl_c();

    tokio::select! {
        res = services => {
            bail!("Services quit unexpectedly {res:?}");
        }
        _ = ctrl_c_signal => {
            info!("Received ctrl+c signal");

            Ok(())
        }
    }

}
