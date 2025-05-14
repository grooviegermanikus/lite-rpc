use quinn::crypto::rustls::QuicServerConfig;
use rustls::ServerConfig;

pub trait ProxyTlsConfigProvider {
    fn get_server_tls_crypto_config(&self) -> QuicServerConfig;
}
