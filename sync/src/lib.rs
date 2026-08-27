pub mod bb;
pub mod cache;
pub mod client;
pub mod commands;
pub mod config;
pub mod domain;
pub mod link;
pub mod socket_server;
pub mod uplink;

pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    #[test]
    fn crypto_provider_install_is_idempotent() {
        crate::install_crypto_provider();
        crate::install_crypto_provider();
    }
}
