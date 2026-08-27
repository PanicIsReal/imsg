pub mod bb;
pub mod cache;
pub mod client;
pub mod commands;
pub mod config;
pub mod domain;
pub mod socket_server;
pub mod uplink;

pub fn install_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");
}
