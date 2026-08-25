use anyhow::{Context, Result};
use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::RootCertStore;
use rustls::ServerConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct TlsMaterial {
    pub server_config: Arc<ServerConfig>,
    pub ca_cert_pem: String,
    pub data_dir: PathBuf,
}

pub fn init_certs(data_dir: &Path) -> Result<TlsMaterial> {
    fs::create_dir_all(data_dir)?;
    let ca_path = data_dir.join("ca.pem");
    let cert_path = data_dir.join("server.pem");
    let key_path = data_dir.join("server-key.pem");
    let clients_dir = data_dir.join("clients");
    fs::create_dir_all(&clients_dir)?;

    if !ca_path.exists() {
        let alg = &rcgen::PKCS_ECDSA_P256_SHA256;
        let ca_kp = rcgen::KeyPair::generate_for(alg)?;
        let mut ca_params = rcgen::CertificateParams::default();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "imsg-bridge-ca");
        let issuer = rcgen::Issuer::from_params(&ca_params, &ca_kp);
        let ca_cert = ca_params.self_signed(&ca_kp)?;
        let ca_pem = ca_cert.pem();

        let server_kp = rcgen::KeyPair::generate_for(alg)?;
        let server_params =
            rcgen::CertificateParams::new(vec!["imsg-bridge.local".to_string()])?;
        let server_cert = server_params.signed_by(&server_kp, &issuer)?;
        let server_pem = server_cert.pem();
        let server_key = server_kp.serialize_pem();

        fs::write(&ca_path, &ca_pem)?;
        fs::write(&cert_path, server_pem)?;
        fs::write(&key_path, server_key)?;
    }

    load_server_config(data_dir)
}

pub fn load_server_config(data_dir: &Path) -> Result<TlsMaterial> {
    let ca_pem = fs::read_to_string(data_dir.join("ca.pem"))?;
    let cert_pem = fs::read_to_string(data_dir.join("server.pem"))?;
    let key_pem = fs::read_to_string(data_dir.join("server-key.pem"))?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .context("parse server cert")?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .context("parse server key")?
        .ok_or_else(|| anyhow::anyhow!("no private key"))?;

    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert?).context("add ca cert")?;
    }

    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .context("client verifier")?;

    let config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .context("server tls config")?;

    Ok(TlsMaterial {
        server_config: Arc::new(config),
        ca_cert_pem: ca_pem,
        data_dir: data_dir.to_path_buf(),
    })
}

pub fn import_client_cert(data_dir: &Path, name: &str, cert_pem: &str) -> Result<PathBuf> {
    let clients_dir = data_dir.join("clients");
    fs::create_dir_all(&clients_dir)?;
    let path = clients_dir.join(format!("{name}.pem"));
    fs::write(&path, cert_pem)?;
    Ok(path)
}

pub fn client_allowed(data_dir: &Path, peer_certs: &[CertificateDer<'_>]) -> bool {
    let clients_dir = data_dir.join("clients");
    let Ok(entries) = fs::read_dir(&clients_dir) else {
        return peer_certs.is_empty();
    };
    if peer_certs.is_empty() {
        return false;
    }
    for entry in entries.flatten() {
        if let Ok(text) = fs::read_to_string(entry.path()) {
            let stored: Vec<CertificateDer<'static>> =
                rustls_pemfile::certs(&mut text.as_bytes())
                    .filter_map(|r| r.ok())
                    .collect();
            for peer in peer_certs {
                for client in &stored {
                    if peer == client {
                        return true;
                    }
                }
            }
        }
    }
    false
}
