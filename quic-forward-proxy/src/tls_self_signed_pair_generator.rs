use std::sync::Arc;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use crate::quic_util::{ALPN_TPU_FORWARDPROXY_PROTOCOL_ID};
use crate::tls_config_provider_client::TpuClientTlsConfigProvider;
use crate::tls_config_provider_server::ProxyTlsConfigProvider;
use rcgen::{generate_simple_self_signed, Certificate, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig};
use rustls::sign::CertifiedKey;
// use rustls::{Certificate, ClientConfig, PrivateKey, ServerConfig};
// use solana_lite_rpc_quic_forward_proxy::skip_server_verification::SkipServerVerification;
// use crate::solana_tls_config::tls_server_config_builder;

impl ProxyTlsConfigProvider for SelfSignedTlsConfigProvider {
    fn get_server_tls_crypto_config(&self) -> QuicServerConfig {
        QuicServerConfig::try_from(self.server_crypto.clone())
            .expect("Failed to convert rustls::ServerConfig to quinn::crypto::rustls::QuicServerConfig")
        // self.server_crypto.clone()
    }
}

impl TpuClientTlsConfigProvider for SelfSignedTlsConfigProvider {
    // fn get_client_tls_crypto_config(&self) -> ClientConfig {
    //     self.client_crypto.clone()
    // }
}

pub struct SelfSignedTlsConfigProvider {
    client_crypto: rustls::ClientConfig,
    server_crypto: rustls::ServerConfig,
}

impl SelfSignedTlsConfigProvider {
    pub fn new_singleton_self_signed_localhost() -> Self {
        // note: this check could be relaxed when you know what you are doing!
        // let
        //     CertifiedKey { cert, key, .. } = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        // 
        // let key = PrivateKeyDer::Pkcs8(key.serialize_der().into());
        // let transport_config = Arc::new(transport_config());
        // let mut server_config =
        //     quinn::ServerConfig::with_single_cert(cert.as_slice(), key).unwrap();
        // server_config.transport_config(transport_config.clone());


        // let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, key).unwrap();
        // server_config.transport = Arc::new(transport_config(opt));


        // let hostnames = vec!["localhost".to_string()];
        // let (certificate, private_key) = Self::gen_tls_certificate_and_key(hostnames);
        // let server_crypto = Self::build_server_crypto(cert, key);
        // Self {
        //     client_crypto: Self::build_client_crypto_insecure(),
        //     server_crypto,
        // }
        todo!()
    }

    // fn gen_tls_certificate_and_key(hostnames: Vec<String>) -> (Certificate, PrivateKey) {
    //     let cert = generate_simple_self_signed(hostnames).unwrap();
    //     let key = cert.serialize_private_key_der();
    //     (Certificate(cert.serialize_der().unwrap()), PrivateKey(key))
    // }

    fn build_client_crypto_insecure() -> ClientConfig {
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

    fn build_server_crypto(server_cert: Certificate, key_pair: KeyPair) -> ServerConfig {
        // let (server_cert, server_key) = gen_tls_certificate_and_key();

        let server_key = PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());

        let mut server_crypto = rustls::ServerConfig::builder()
            // FIXME we want client auth
            // .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(vec![server_cert.der().clone()], server_key)
            .unwrap();
        server_crypto.alpn_protocols = vec![ALPN_TPU_FORWARDPROXY_PROTOCOL_ID.to_vec()];
        server_crypto
    }

    // pub fn get_client_tls_crypto_config(&self) -> QuicClientConfig {
    //     &self.client_crypto
    // }

}


fn transport_config() -> quinn::TransportConfig {
    // High stream windows are chosen because the amount of concurrent streams
    // is configurable as a parameter.
    let mut config = quinn::TransportConfig::default();
    // TODO tune
    // config.max_concurrent_uni_streams(opt.max_streams.try_into().unwrap());
    // config.initial_mtu(opt.initial_mtu);

    let mut acks = quinn::AckFrequencyConfig::default();
    acks.ack_eliciting_threshold(10u32.into());
    config.ack_frequency_config(Some(acks));

    config
}
