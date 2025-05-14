use rustls::crypto::CryptoProvider;
use rustls::NamedGroup;
use {
    rustls::{
        client::WantsClientCert, server::WantsServerCert, ClientConfig, ConfigBuilder, ServerConfig,
    },
    std::sync::Arc,
};
use crate::skip_server_verification::SkipServerVerification;
use crate::skip_client_verification::SkipClientVerification;

pub fn tls_client_config_builder() -> ConfigBuilder<ClientConfig, WantsClientCert> {
    ClientConfig::builder_with_provider(Arc::new(crypto_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
}

pub fn tls_server_config_builder() -> ConfigBuilder<ServerConfig, WantsServerCert> {
    ServerConfig::builder_with_provider(Arc::new(crypto_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(SkipClientVerification::new())
}

fn crypto_provider() -> CryptoProvider {
    let mut provider = rustls::crypto::ring::default_provider();
    // Disable all key exchange algorithms except X25519
    provider
        .kx_groups
        .retain(|kx| kx.name() == NamedGroup::X25519);
    provider
}

