use rcgen::generate_simple_self_signed;
use rustls::{Certificate, ClientConfig, PrivateKey};
use solana_lite_rpc_core::quic_connection_utils::SkipServerVerification;
use crate::quic_util::ALPN_TPU_FORWARDPROXY_PROTOCOL_ID;

pub trait ProxyTlsConfigProvider {

    fn get_client_tls_crypto_config(&self) -> ClientConfig;

}

impl ProxyTlsConfigProvider for SelfSignedTlsConfiguration {
    fn get_client_tls_crypto_config(&self) -> ClientConfig {
        self.client_crypto.clone()
    }
}

pub struct SelfSignedTlsConfiguration {
    hostnames: Vec<String>,
    certificate: Certificate,
    private_key: PrivateKey,
    client_crypto: ClientConfig,
}

impl SelfSignedTlsConfiguration {
    pub fn new_self_signed_localhost() -> Self {
        let hostnames = vec!["localhost".to_string()];
        let (certificate, private_key) = Self::gen_server_cert_key(hostnames.clone());
        Self {
            hostnames: hostnames,
            certificate,
            private_key,
            client_crypto: Self::build_client_crypto(),
        }
    }

    fn gen_server_cert_key(hostnames: Vec<String>) -> (Certificate, PrivateKey) {
        let cert = generate_simple_self_signed(hostnames).unwrap();
        let key = cert.serialize_private_key_der();
        (Certificate(cert.serialize_der().unwrap()), PrivateKey(key))
    }

    fn build_client_crypto() -> ClientConfig {
        let mut client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            // .with_root_certificates(roots)
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();
        client_crypto.enable_early_data = true;
        client_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];
        return client_crypto;
    }

    pub fn get_client_tls_crypto_config(&self) -> &ClientConfig {
        &self.client_crypto
    }

}

