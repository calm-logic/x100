//! The certificate authority the proxy ends TLS with, and the TLS settings on both sides.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

use crate::VaultError;
use crate::seal::{self, Sealer};

const CERT_FILE: &str = "authority.pem";
const KEY_FILE: &str = "authority.key.sealed";
const KEY_KIND: &str = "authority-key";

/// Leaf certificates are short-lived and minted again before they run out, so a long-running
/// daemon never serves an expired one.
const LEAF_LIFE: Duration = Duration::days(30);
const LEAF_RENEW: Duration = Duration::days(1);

/// Only HTTP/1.1 is offered to clients on the intercepted side.
const ALPN: &[u8] = b"http/1.1";

/// The proxy's certificate authority. Its public certificate is a file that sandboxes, and only
/// sandboxes, are told to trust; its private key is sealed with the vault's key.
pub struct Authority {
    cert_path: PathBuf,
    cert: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
    /// One key for every leaf: each host still gets its own certificate, and making a key per
    /// host would only slow the first request to it.
    leaf_key: KeyPair,
    leaves: Mutex<HashMap<String, (Arc<ServerConfig>, OffsetDateTime)>>,
}

impl Authority {
    pub(crate) fn load_or_create(dir: &Path, sealer: &Sealer) -> Result<Self, VaultError> {
        let key_path = dir.join(KEY_FILE);
        let cert_path = dir.join(CERT_FILE);
        let key = match std::fs::read(&key_path) {
            Ok(sealed) => {
                let der = sealer.open(KEY_KIND, &key_path, &sealed)?;
                KeyPair::try_from(der.as_slice()).map_err(cert_error)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = KeyPair::generate().map_err(cert_error)?;
                let der = Zeroizing::new(key.serialize_der());
                seal::write_atomic(&key_path, &sealer.seal(KEY_KIND, &der)?, 0o600)?;
                // A new key makes any old certificate useless.
                let _ = std::fs::remove_file(&cert_path);
                key
            }
            Err(e) => return Err(VaultError::Io(key_path, e)),
        };
        let params = authority_params(&key);
        let pem = match std::fs::read_to_string(&cert_path) {
            Ok(pem) => pem,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let pem = params.self_signed(&key).map_err(cert_error)?.pem();
                seal::write_atomic(&cert_path, pem.as_bytes(), 0o644)?;
                pem
            }
            Err(e) => return Err(VaultError::Io(cert_path, e)),
        };
        let cert = pem_cert(&pem).ok_or_else(|| {
            VaultError::Corrupt(format!("{} holds no certificate", cert_path.display()))
        })?;
        Ok(Self {
            cert_path,
            cert,
            issuer: Issuer::new(params, key),
            leaf_key: KeyPair::generate().map_err(cert_error)?,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// The authority's public certificate in PEM, for `NODE_EXTRA_CA_CERTS`,
    /// `CODEX_CA_CERTIFICATE`, `SSL_CERT_FILE` or `curl --cacert` inside a sandbox.
    pub fn certificate_path(&self) -> &Path {
        &self.cert_path
    }

    pub fn certificate(&self) -> &CertificateDer<'static> {
        &self.cert
    }

    /// The server side of an intercepted connection to `host`.
    pub(crate) fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, VaultError> {
        let now = OffsetDateTime::now_utc();
        let mut leaves = self.leaves.lock().expect("leaf cache");
        if let Some((config, renew)) = leaves.get(host)
            && now < *renew
        {
            return Ok(Arc::clone(config));
        }
        let mut params = CertificateParams::new(vec![host.to_owned()]).map_err(cert_error)?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.not_before = now - Duration::days(1);
        params.not_after = now + LEAF_LIFE;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let leaf = params
            .signed_by(&self.leaf_key, &self.issuer)
            .map_err(cert_error)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.serialize_der()));
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("ring supports the default versions")
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone(), self.cert.clone()], key)
            .map_err(|e| VaultError::Certificate(e.to_string()))?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        let config = Arc::new(config);
        leaves.insert(
            host.to_owned(),
            (Arc::clone(&config), params.not_after - LEAF_RENEW),
        );
        Ok(config)
    }
}

fn authority_params(key: &KeyPair) -> CertificateParams {
    let mut params = CertificateParams::default();
    // Named after its own key, so two installs' authorities are told apart in any trust store.
    let id = seal::hex(&key.public_key_raw()[1..9]);
    params
        .distinguished_name
        .push(DnType::CommonName, format!("x100 sandbox authority {id}"));
    params
        .distinguished_name
        .push(DnType::OrganizationName, "x100");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(3650);
    params
}

fn pem_cert(pem: &str) -> Option<CertificateDer<'static>> {
    use rustls::pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(pem.as_bytes()).ok()
}

fn cert_error(e: rcgen::Error) -> VaultError {
    VaultError::Certificate(e.to_string())
}

pub(crate) fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Which authorities the proxy trusts for the real upstream. Certificate checking is always on.
#[derive(Debug, Clone)]
pub enum UpstreamTrust {
    /// The operating system's trust store.
    System,
    /// Only these, for tests against a local server with a test authority.
    Only(Vec<CertificateDer<'static>>),
}

impl UpstreamTrust {
    /// Only the certificates in a PEM file, for a test's daemon talking to its own servers.
    pub fn from_pem_file(path: &Path) -> Result<UpstreamTrust, VaultError> {
        use rustls::pki_types::pem::PemObject;
        let certs = CertificateDer::pem_file_iter(path)
            .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
            .map_err(|e| VaultError::Certificate(format!("{}: {e}", path.display())))?;
        if certs.is_empty() {
            return Err(VaultError::Certificate(format!(
                "{} holds no certificate",
                path.display()
            )));
        }
        Ok(UpstreamTrust::Only(certs))
    }
}

pub(crate) fn client_config(trust: &UpstreamTrust) -> Result<Arc<ClientConfig>, VaultError> {
    let mut roots = RootCertStore::empty();
    match trust {
        UpstreamTrust::System => {
            let found = rustls_native_certs::load_native_certs();
            let (added, _) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err(VaultError::Certificate(format!(
                    "no trusted authorities found on this system: {:?}",
                    found.errors
                )));
            }
        }
        UpstreamTrust::Only(certs) => {
            for cert in certs {
                roots
                    .add(cert.clone())
                    .map_err(|e| VaultError::Certificate(e.to_string()))?;
            }
        }
    }
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(config))
}
