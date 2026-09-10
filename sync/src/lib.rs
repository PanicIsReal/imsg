pub mod cache;
pub mod client;
pub mod commands;
pub mod config;
pub mod socket_server;
pub mod uplink;

pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
