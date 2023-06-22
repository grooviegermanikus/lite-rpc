// DEPRECATED: use quic-proxy main.rs

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use anyhow::anyhow;
use log::info;
use rcgen::IsCa::SelfSignedOnly;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use lite_rpc_quic_forward_proxy::quic_util::ALPN_TPU_FORWARDPROXY_PROTOCOL_ID;
use solana_lite_rpc_core::quic_connection_utils::SkipServerVerification;

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    // FIXME configured insecure https://quinn-rs.github.io/quinn/quinn/certificate.html
    let mut _roots = rustls::RootCertStore::empty();
    // TODO add certs

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        // .with_root_certificates(roots)
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    client_crypto.enable_early_data = true;
    client_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(client_crypto)));

    let connection_timeout = Duration::from_secs(5);
    let connecting = endpoint.connect("127.0.0.1:8080".parse().unwrap(), "localhost").unwrap();
    let connection = timeout(connection_timeout, connecting).await??;

    let (mut send, mut recv)  = connection.open_bi().await?;

    if false { // Rebind
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        info!("rebinding to {}", addr);
        endpoint.rebind(socket).expect("rebind failed");
    }

    let request = "FOO BAR";

    send.write_all(request.as_bytes())
        .await
        .map_err(|e| anyhow!("failed to send request: {}", e))?;
    send.finish()
        .await
        .map_err(|e| anyhow!("failed to shutdown stream: {}", e))?;
    let resp = recv
        .read_to_end(usize::MAX)
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;

    info!("resp: {:?}", std::str::from_utf8(&resp));

    connection.close(99u32.into(), b"done");

    // Give the server a fair chance to receive the close packet
    endpoint.wait_idle().await;


    Ok(())
}
