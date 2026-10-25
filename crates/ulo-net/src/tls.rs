use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::pem::{self, PemObject};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// A server's TLS certificate chain and private key, loaded with rustls.
///
/// Nothing is read or parsed where a `Tls` is built: the server calls [`load`](Self::load) in its
/// `prepare`, so an unreadable file, a bad certificate or a key that does not match it fails
/// startup as `StartupError::Configure` beside every other configuration error, never inside the
/// serve loop. ALPN is set per transport at load: `h2` and `http/1.1` for HTTP, `h2` for gRPC.
#[derive(Clone)]
pub struct Tls {
    source: Source,
}

#[derive(Clone)]
enum Source {
    PemFiles { cert: PathBuf, key: PathBuf },
    Pem { cert: Vec<u8>, key: Vec<u8> },
}

impl Tls {
    /// PEM files: the certificate chain, leaf first, and its private key.
    pub fn from_pem_files(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Tls {
        Tls { source: Source::PemFiles { cert: cert.into(), key: key.into() } }
    }

    /// PEM bytes already in memory.
    pub fn from_pem(cert: impl Into<Vec<u8>>, key: impl Into<Vec<u8>>) -> Tls {
        Tls { source: Source::Pem { cert: cert.into(), key: key.into() } }
    }

    /// Reads and parses the certificate and key, checks that they match, and builds the server
    /// configuration with `alpn` as the protocols offered, in preference order. The server's
    /// runtime crate builds its acceptor from it.
    pub fn load(&self, alpn: &[&[u8]]) -> Result<Arc<ServerConfig>, TlsError> {
        let (certs, key) = match &self.source {
            Source::PemFiles { cert, key } => (
                certificates(&read(cert)?).map_err(|cause| TlsError::Certificate { path: Some(cert.clone()), cause })?,
                private_key(&read(key)?).map_err(|cause| TlsError::PrivateKey { path: Some(key.clone()), cause })?,
            ),
            Source::Pem { cert, key } => (
                certificates(cert).map_err(|cause| TlsError::Certificate { path: None, cause })?,
                private_key(key).map_err(|cause| TlsError::PrivateKey { path: None, cause })?,
            ),
        };
        // An explicit provider rather than the process default, which panics when the build
        // enables both rustls backends and nothing installed one.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(TlsError::Rustls)?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|error| match error {
                rustls::Error::InconsistentKeys(_) => {
                    TlsError::PrivateKey { path: key_path(&self.source), cause: KeyCause::DoesNotMatchCertificate }
                }
                error => TlsError::Rustls(error),
            })?;
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        Ok(Arc::new(config))
    }
}

fn key_path(source: &Source) -> Option<PathBuf> {
    match source {
        Source::PemFiles { key, .. } => Some(key.clone()),
        Source::Pem { .. } => None,
    }
}

fn read(path: &Path) -> Result<Vec<u8>, TlsError> {
    fs::read(path).map_err(|source| TlsError::Read { path: path.to_owned(), source })
}

fn certificates(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, CertificateCause> {
    let mut certs = Vec::new();
    for (index, cert) in CertificateDer::pem_slice_iter(bytes).enumerate() {
        match cert {
            Ok(cert) => certs.push(cert),
            Err(error) => return Err(CertificateCause::Unparseable { index, reason: error.to_string() }),
        }
    }
    if certs.is_empty() {
        return Err(CertificateCause::NoneFound);
    }
    Ok(certs)
}

fn private_key(bytes: &[u8]) -> Result<PrivateKeyDer<'static>, KeyCause> {
    PrivateKeyDer::from_pem_slice(bytes).map_err(|error| match error {
        pem::Error::NoItemsFound => KeyCause::NoneFound,
        error => KeyCause::Unparseable { reason: error.to_string() },
    })
}

/// Why a certificate PEM was refused.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertificateCause {
    /// The PEM holds no `CERTIFICATE` section.
    NoneFound,
    /// The `index`th section, counting from 0, did not parse.
    Unparseable { index: usize, reason: String },
}

impl fmt::Display for CertificateCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertificateCause::NoneFound => f.write_str("no `CERTIFICATE` section found"),
            CertificateCause::Unparseable { index, reason } => write!(f, "section {index} does not parse: {reason}"),
        }
    }
}

/// Why a private key PEM was refused.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCause {
    /// The PEM holds no private key section.
    NoneFound,
    /// The key section did not parse.
    Unparseable { reason: String },
    /// The key parses and is not the certificate's.
    DoesNotMatchCertificate,
}

impl fmt::Display for KeyCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyCause::NoneFound => f.write_str("no private key section found"),
            KeyCause::Unparseable { reason } => write!(f, "does not parse: {reason}"),
            KeyCause::DoesNotMatchCertificate => f.write_str("does not match the certificate"),
        }
    }
}

/// Why a server's TLS configuration could not be loaded.
#[non_exhaustive]
#[derive(Debug)]
pub enum TlsError {
    /// A certificate or key file could not be read; `source.kind()` says why.
    Read { path: PathBuf, source: io::Error },
    /// The certificate PEM holds no certificate, or one that does not parse. `path` is the file it
    /// was read from, `None` for PEM given in memory.
    Certificate { path: Option<PathBuf>, cause: CertificateCause },
    /// The key PEM holds no private key, one that does not parse, or one that is not the
    /// certificate's.
    PrivateKey { path: Option<PathBuf>, cause: KeyCause },
    /// rustls refused the configuration for another reason, such as a key of an unsupported type.
    Rustls(rustls::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Read { path, source } => write!(f, "cannot read `{}`: {source}", path.display()),
            TlsError::Certificate { path: Some(path), cause } => write!(f, "bad TLS certificate: `{}`: {cause}", path.display()),
            TlsError::Certificate { path: None, cause } => write!(f, "bad TLS certificate: {cause}"),
            TlsError::PrivateKey { path: Some(path), cause } => write!(f, "bad TLS private key: `{}`: {cause}", path.display()),
            TlsError::PrivateKey { path: None, cause } => write!(f, "bad TLS private key: {cause}"),
            TlsError::Rustls(error) => write!(f, "TLS configuration refused: {error}"),
        }
    }
}

impl Error for TlsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            TlsError::Read { source, .. } => Some(source),
            TlsError::Rustls(error) => Some(error),
            _ => None,
        }
    }
}
