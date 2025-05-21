use std::error::Error;
use crate::proxy_request_format::TpuForwardingRequest;
use crate::quic_util::{apply_gso_workaround, connection_stats, ALPN_TPU_FORWARDPROXY_PROTOCOL_ID};
use crate::shared::ForwardPacket;
use crate::util::FALLBACK_TIMEOUT;
use anyhow::{anyhow, bail, Context};
use log::{debug, error, info, trace, warn};
use quinn::{Connecting, Endpoint, ServerConfig, VarInt};
use solana_sdk::packet::PACKET_DATA_SIZE;
use std::net::{SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use quinn::crypto::rustls::QuicServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::sync::mpsc::Sender;

// note: setting this to "1" did not make a difference!
// solana server sets this to 256
const MAX_CONCURRENT_UNI_STREAMS: u32 = 24;

pub struct ProxyListener {
    proxy_listener_addr: SocketAddr,
}

impl ProxyListener {
    pub fn new(
        proxy_listener_addr: SocketAddr,
    ) -> Self {
        Self {
            proxy_listener_addr,
        }
    }

    pub async fn listen(&self, forwarder_channel: &Sender<ForwardPacket>) -> anyhow::Result<()> {
        info!(
            "TPU Quic Proxy server listening on {}",
            self.proxy_listener_addr
        );

        let endpoint =
            Self::new_proxy_listen_server_endpoint(self.proxy_listener_addr)
                .await;

        while let Some(incoming) = endpoint.accept().await {
            let _remote_addr: SocketAddr = incoming.remote_address();
            let connecting = incoming.accept().unwrap(); // TODO unwrap
            let forwarder_channel_copy = forwarder_channel.clone();
            tokio::spawn(async move {
                match Self::handle_client_connection(connecting, forwarder_channel_copy).await {
                    Ok(()) => {
                        debug!("connection handled correctly");
                    }
                    Err(err) => {
                        error!(
                            "failed handling connection from client: {reason} - skip",
                            reason = err
                        );
                    }
                }
            });
        }

        bail!("TPU Quic Proxy server stopped");
    }



    async fn new_proxy_listen_server_endpoint(
        // tls_config: &SelfSignedTlsConfigProvider,
        proxy_listener_addr: SocketAddr,
    ) -> Endpoint {
        // let server_tls_config: QuicServerConfig = tls_config.get_server_tls_crypto_config();

        /*

            let mut server_crypto = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)?;
            server_crypto.alpn_protocols = common::ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();
            if options.keylog {
                server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());
            }

            let mut server_config =
                quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));
            let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
            transport_config.max_concurrent_uni_streams(0_u8.into());

         */

        let mut quinn_server_config = configure_server().unwrap();
        // let server_tls_config = QuicServerConfig::try_from(server_config).unwrap();
        // let mut quinn_server_config: quinn::ServerConfig = ServerConfig::with_crypto(Arc::new(server_tls_config));


        // note: this config must be aligned with lite-rpc's client config
        let transport_config = Arc::get_mut(&mut quinn_server_config.transport).unwrap();
        transport_config.max_concurrent_uni_streams(VarInt::from_u32(MAX_CONCURRENT_UNI_STREAMS));
        // no bidi streams used
        transport_config.max_concurrent_bidi_streams(VarInt::from_u32(0));
        let timeout = Duration::from_secs(10).try_into().unwrap();
        transport_config.max_idle_timeout(Some(timeout));
        transport_config.keep_alive_interval(Some(Duration::from_millis(500)));
        transport_config.stream_receive_window((PACKET_DATA_SIZE as u32).into());
        transport_config
            .receive_window((PACKET_DATA_SIZE as u32 * MAX_CONCURRENT_UNI_STREAMS).into());
        apply_gso_workaround(transport_config); // TODO

        Endpoint::server(quinn_server_config, proxy_listener_addr).unwrap()
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn handle_client_connection(
        connecting: Connecting,
        forwarder_channel: Sender<ForwardPacket>,
    ) -> anyhow::Result<()> {
        let client_connection = connecting.await.context("handshake")?;

        debug!(
            "inbound connection established, client {}",
            client_connection.remote_address()
        );

        loop {
            let maybe_stream = client_connection.accept_uni().await;
            match maybe_stream {
                Ok(mut recv_stream) => {
                    let forwarder_channel_copy = forwarder_channel.clone();
                    tokio::spawn(async move {
                        let raw_request = recv_stream.read_to_end(10_000_000).await.unwrap();

                        let proxy_request =
                            TpuForwardingRequest::try_deserialize_from_wire_format(&raw_request)
                                .unwrap();

                        if proxy_request.get_tpu_nodes().is_empty() {
                            warn!("no tpu nodes in request - skip");
                            return;
                        }

                        trace!("proxy request details: {}", proxy_request);
                        let txs = proxy_request.get_transaction_bytes();

                        debug!(
                            "enqueue transaction batch of size {} to {} tpu nodes",
                            txs.len(),
                            proxy_request.get_tpu_nodes().len(),
                        );
                        if forwarder_channel_copy.capacity() < forwarder_channel_copy.max_capacity()
                        {
                            debug!(
                                "forward channel buffered: capacity {} of {}",
                                forwarder_channel_copy.capacity(),
                                forwarder_channel_copy.max_capacity()
                            );
                        }

                        for tpu_node in proxy_request.get_tpu_nodes() {
                            let tpu_address = tpu_node.tpu_socket_addr;
                            forwarder_channel_copy
                                .send_timeout(
                                    ForwardPacket::new(
                                        txs.clone(),
                                        tpu_address,
                                        proxy_request.get_hash(),
                                    ),
                                    FALLBACK_TIMEOUT,
                                )
                                .await
                                .context("sending internal packet from proxy to forwarder")
                                .unwrap();
                        }
                    });

                    debug!(
                        "Inbound connection stats: {}",
                        connection_stats(&client_connection)
                    );
                }
                Err(quinn::ConnectionError::ApplicationClosed(reason)) => {
                    debug!("connection closed by client - reason: {:?}", reason);
                    if reason.error_code != VarInt::from_u32(0) {
                        return Err(anyhow!(
                            "connection closed by client with unexpected reason: {:?}",
                            reason
                        ));
                    }
                    debug!("connection gracefully closed by client");
                    return Ok(());
                }
                Err(e) => {
                    error!("failed to accept stream: {}", e);
                    bail!("error accepting stream");
                }
            }; // -- result
        } // -- loop
    }
}


// TODO move
// from quinn examples
fn configure_server()
    -> Result<ServerConfig, Box<dyn Error + Send + Sync + 'static>> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = CertificateDer::from(cert.cert);
    // let priv_key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let private_key =  PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], private_key)?;
    server_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];

    let server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

    Ok(server_config)
}