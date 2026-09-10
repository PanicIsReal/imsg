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
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    #[test]
    fn install_crypto_provider_can_run_twice() {
        super::install_crypto_provider();
        super::install_crypto_provider();
    }
}
