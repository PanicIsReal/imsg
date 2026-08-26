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

pub fn init_certs(data_dir: &Path, bind: &str) -> Result<TlsMaterial> {
    fs::create_dir_all(data_dir)?;
    let ca_path = data_dir.join("ca.pem");
    let ca_key_path = data_dir.join("ca-key.pem");
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
        let mut sans = vec!["imsg-bridge.local".to_string(), "localhost".to_string()];
        if !bind.is_empty() && bind != "0.0.0.0" {
            sans.push(bind.to_string());
        }
        if let Ok(host) = hostname::get()
            .and_then(|h| h.into_string().map_err(|_| std::io::Error::other("hostname")))
        {
            if !sans.contains(&host) {
                sans.push(host);
            }
        }
        let server_params = rcgen::CertificateParams::new(sans)?;
        let server_cert = server_params.signed_by(&server_kp, &issuer)?;

        fs::write(&ca_path, &ca_pem)?;
        fs::write(&ca_key_path, ca_kp.serialize_pem())?;
        fs::write(&cert_path, server_cert.pem())?;
        fs::write(&key_path, server_kp.serialize_pem())?;
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

/// Pairing enrollment is server-auth only (no client cert).
pub fn load_enroll_tls_config(data_dir: &Path) -> Result<ServerConfig> {
    let cert_pem = fs::read_to_string(data_dir.join("server.pem"))?;
    let key_pem = fs::read_to_string(data_dir.join("server-key.pem"))?;
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .context("parse enroll cert")?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .context("parse enroll key")?
        .ok_or_else(|| anyhow::anyhow!("no private key"))?;
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("enroll tls config")
}

pub fn sign_client_csr(data_dir: &Path, name: &str, csr_pem: &str) -> Result<String> {
    let ca_pem = fs::read_to_string(data_dir.join("ca.pem")).context("read ca")?;
    let ca_key_pem = fs::read_to_string(data_dir.join("ca-key.pem")).context("read ca key")?;
    let ca_kp = rcgen::KeyPair::from_pem(&ca_key_pem).context("parse ca key")?;
    let issuer = rcgen::Issuer::from_ca_cert_pem(&ca_pem, &ca_kp).context("ca issuer")?;
    let csr = rcgen::CertificateSigningRequestParams::from_pem(csr_pem).context("parse csr")?;
    let cert = csr.signed_by(&issuer).context("sign csr")?;
    let pem = cert.pem();
    let clients_dir = data_dir.join("clients");
    fs::create_dir_all(&clients_dir)?;
    fs::write(clients_dir.join(format!("{name}.pem")), &pem)?;
    Ok(pem)
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
            let stored: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut text.as_bytes())
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
