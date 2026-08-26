pub mod attachments;
pub mod commands;
pub mod config;
pub mod contacts;
pub mod imsg_rpc;
pub mod mdns_advertise;
pub mod pairing;
pub mod server;
pub mod steipete;
pub mod tls;

pub fn install_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring crypto provider");
}
