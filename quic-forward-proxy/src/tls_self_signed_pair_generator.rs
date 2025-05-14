use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use crate::quic_util::{ALPN_TPU_FORWARDPROXY_PROTOCOL_ID};
use crate::tls_config_provider_client::TpuClientTlsConfigProvider;
use crate::tls_config_provider_server::ProxyTlsConfigProvider;
use rcgen::generate_simple_self_signed;
// use rustls::{Certificate, ClientConfig, PrivateKey, ServerConfig};
// use solana_lite_rpc_quic_forward_proxy::skip_server_verification::SkipServerVerification;
// use crate::solana_tls_config::tls_server_config_builder;

impl ProxyTlsConfigProvider for SelfSignedTlsConfigProvider {
    fn get_server_tls_crypto_config(&self) -> QuicServerConfig {
        self.server_crypto.clone()
    }
}

impl TpuClientTlsConfigProvider for SelfSignedTlsConfigProvider {
    // fn get_client_tls_crypto_config(&self) -> ClientConfig {
    //     self.client_crypto.clone()
    // }
}

pub struct SelfSignedTlsConfigProvider {
    client_crypto: rustls::ClientConfig,
    server_crypto: QuicServerConfig,
}

impl SelfSignedTlsConfigProvider {
    pub fn new_singleton_self_signed_localhost() -> Self {
        // // note: this check could be relaxed when you know what you are doing!
        // let hostnames = vec!["localhost".to_string()];
        // let (certificate, private_key) = Self::gen_tls_certificate_and_key(hostnames);
        // let server_crypto = Self::build_server_crypto(certificate, private_key);
        // Self {
        //     client_crypto: Self::build_client_crypto_insecure(),
        //     server_crypto,
        // }
        todo!()
    }

    // fn gen_tls_certificate_and_key(hostnames: Vec<String>) -> (Certificate, PrivateKey) {
        // let cert = generate_simple_self_signed(hostnames).unwrap();
        // let key = cert.serialize_private_key_der();
        // (Certificate(cert.serialize_der().unwrap()), PrivateKey(key))
    // }

    fn build_client_crypto_insecure() -> QuicClientConfig {
        todo!()
        // let mut client_crypto = rustls::ClientConfig::builder()
        //     .with_safe_defaults()
        //     // .with_root_certificates(roots)
        //     .with_custom_certificate_verifier(SkipServerVerification::new())
        //     .with_no_client_auth();
        // client_crypto.enable_early_data = true;
        // client_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];
        // client_crypto
    }

    // fn build_server_crypto(server_cert: Certificate, server_key: PrivateKey) -> ServerConfig {
        // let (server_cert, server_key) = gen_tls_certificate_and_key();

        // let mut server_crypto = rustls::ServerConfig::builder()
        //     // FIXME we want client auth
        //     .with_safe_defaults()
        //     .with_no_client_auth()
        //     .with_single_cert(vec![server_cert], server_key)
        //     .unwrap();
        // server_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];
        // server_crypto


        // todo!()
    // }

    // pub fn get_client_tls_crypto_config(&self) -> QuicClientConfig {
    //     &self.client_crypto
    // }
}
