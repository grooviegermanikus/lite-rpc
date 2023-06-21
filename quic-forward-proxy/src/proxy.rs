use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use anyhow::bail;
use log::{error, info};
use quinn::{Connecting, Endpoint, ServerConfig};
use rustls::server::ResolvesServerCert;
use solana_sdk::signature::Keypair;
use solana_sdk::transaction::VersionedTransaction;
use tokio::net::ToSocketAddrs;
use solana_lite_rpc_core::AnyhowJoinHandle;
use solana_streamer::tls_certificates::new_self_signed_tls_certificate;

struct Fail;
impl ResolvesServerCert for Fail {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
}

pub struct QuicForwardProxy {


}

impl QuicForwardProxy {
    pub async fn new() -> anyhow::Result<Self> {

        Ok(Self {})

    }

    pub async fn start_services(
        mut self,
    ) -> anyhow::Result<()> {
        let tx_sample_sender: AnyhowJoinHandle = tokio::spawn(async move {
            info!("Sample TX Server start");

            let identity_keypair = Keypair::new(); // TODO
            let ip_addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

            let (cert, priv_key) = new_self_signed_tls_certificate(&identity_keypair, ip_addr).unwrap();




            let mut server_tls_config = rustls::ServerConfig::builder()
                // FIXME we want client auth
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(Fail));

            let mut quinn_server_config = ServerConfig::with_crypto(Arc::new(server_tls_config));

            let socket = SocketAddr::new(IpAddr::V4("127.0.0.1".parse().unwrap()), 8080);
            let endpoint = Endpoint::server(quinn_server_config, socket).unwrap();
            info!("listening on {}", endpoint.local_addr()?);

            while let Some(conn) = endpoint.accept().await {
                info!("connection incoming");
                // let fut = handle_connection(conn);
                tokio::spawn(async move {
                    // if let Err(e) = fut.await {
                    //     error!("connection failed: {reason}", reason = e.to_string())
                    // }
                });
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

fn handle_connection(p0: Connecting) -> () {
    todo!()
}

