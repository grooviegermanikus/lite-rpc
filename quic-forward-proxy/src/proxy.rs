use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use anyhow::{anyhow, bail};
use log::{error, info, warn};
use quinn::{Connecting, Endpoint, SendStream, ServerConfig};
use rcgen::generate_simple_self_signed;
use rustls::{Certificate, PrivateKey};
use rustls::server::ResolvesServerCert;
use solana_sdk::signature::Keypair;
use solana_sdk::transaction::VersionedTransaction;
use tokio::net::ToSocketAddrs;
use solana_lite_rpc_core::AnyhowJoinHandle;
use solana_streamer::tls_certificates::new_self_signed_tls_certificate;
use lite_rpc_quic_forward_proxy::quic_util::ALPN_TPU_FORWARDPROXY_PROTOCOL_ID;


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
            // let ip_addr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));


            let (server_cert, server_key) = gen_server_cert_key();

            let mut server_tls_config = rustls::ServerConfig::builder()
                // FIXME we want client auth
                .with_safe_defaults()
                .with_no_client_auth()
                .with_single_cert(vec![server_cert], server_key)?;
            server_tls_config.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];

            let mut quinn_server_config = ServerConfig::with_crypto(Arc::new(server_tls_config));

            let socket = SocketAddr::new(IpAddr::V4("127.0.0.1".parse().unwrap()), 8080);
            let endpoint = Endpoint::server(quinn_server_config, socket).unwrap();
            info!("listening on {}", endpoint.local_addr()?);

            while let Some(conn) = endpoint.accept().await {
                info!("connection incoming");
                let fut = handle_connection2(conn);
                tokio::spawn(async move {
                    if let Err(e) = fut.await {
                        error!("connection failed: {reason}", reason = e.to_string())
                    }
                });
            }

            // while let Some(conn) = endpoint.accept().await {
            //     info!("connection incoming");
            //     // let fut = handle_connection(conn);
            //     tokio::spawn(async move {
            //         info!("start thread");
            //         handle_connection2(conn).await.unwrap();
            //         // if let Err(e) = fut.await {
            //         //     error!("connection failed: {reason}", reason = e.to_string())
            //         // }
            //     });
            // }

            bail!("Sample TX server stopped");
        });

        tokio::select! {
            res = tx_sample_sender => {
                bail!("Sample TX server exited unexpectedly {res:?}");
            },
        }
    }

}

fn gen_server_cert_key() -> (Certificate, PrivateKey) {
    info!("generating self-signed certificate");
    let cert = generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let key = cert.serialize_private_key_der();
    (Certificate(cert.serialize_der().unwrap()), PrivateKey(key))
}

// meins
async fn handle_connection2(connecting: Connecting) -> anyhow::Result<()> {
    let connection = connecting.await?;
    info!("connection established, remote {connection}", connection = connection.remote_address());

    info!("established");
    async {
        loop {
            let stream = connection.accept_bi().await;
            let (mut send, recv) = match stream {
                Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                    info!("connection closed");
                    return Ok(());
                }
                Err(e) => {
                    warn!("connection failed: {}", e);
                    return Err(anyhow::Error::msg("connection failed"));
                }
                Ok(s) => s,
            };
            tokio::spawn(async move {
                let result = recv.read_to_end(100000).await
                    .unwrap();
                let str = std::str::from_utf8(&result).unwrap();
                info!("read result {:?}", str);
                send_data(send).await;
            });
            // info!("stream okey {:?}", stream);
            // let fut = handle_request2(stream).await;
            // tokio::spawn(
            //     async move {
            //         if let Err(e) = fut.await {
            //             error!("failed: {reason}", reason = e.to_string());
            //         }
            //     }
            // );
        }
    }
        .await?;
    Ok(())
}

async fn send_data(mut send: SendStream) -> anyhow::Result<()> {
    send.write_all(b"HELLO STRANGER\r\n").await?;
    send.finish().await?;
    Ok(())
}

async fn handle_request2(
    (mut send, recv): (quinn::SendStream, quinn::RecvStream),
) -> anyhow::Result<()> {
    info!("handle incoming request...");

    send.write_all(b"HELLO STRANGER\r\n").await?;
    send.finish().await?;

    Ok(())
}
