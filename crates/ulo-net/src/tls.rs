use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::pem::{self, PemObject};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// A server's TLS certificate chain and private key, loaded with rustls, or a rustls
/// configuration the caller built.
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
    Config(Arc<ServerConfig>),
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

    /// A rustls configuration built by the caller, for what the PEM constructors cannot say:
    /// verifying client certificates (`with_client_cert_verifier`), a certificate resolver that
    /// picks by SNI, session tickets, a crypto provider other than the default.
    ///
    /// The server still sets the ALPN protocols its transport needs, so HTTP/2 is not switched off
    /// by accident. At [`load`](Self::load) the given `alpn_protocols` are read against the
    /// protocols the transport needs (`h2` and `http/1.1` for HTTP):
    ///
    /// - empty: the server sets the protocols it needs;
    /// - naming every protocol the transport needs: accepted, and the server offers exactly the
    ///   transport's protocols in its order. A further protocol the configuration names
    ///   (`acme-tls/1`, `h3`) is dropped from that list, since a client that negotiated it
    ///   would reach a server that cannot speak it; a client offering only such a protocol gets a
    ///   TLS alert at the handshake;
    /// - missing one the transport needs, `h2` only or `http/1.1` only included: refused as
    ///   [`TlsError::Alpn`], which `listen` reports as `StartupError::Configure`.
    ///
    /// A transport that needs no protocol (the TCP link) takes the configuration as given. The
    /// handed `Arc` is never modified: a configuration whose protocols change is copied, with
    /// everything else it set, and one whose list already equals the transport's is used as given.
    pub fn from_config(config: Arc<ServerConfig>) -> Tls {
        Tls { source: Source::Config(config) }
    }

    /// Reads and parses the certificate and key, checks that they match, and builds the server
    /// configuration with `alpn` as the protocols offered, in preference order. For a [`Tls`]
    /// made by [`from_config`](Self::from_config) it reconciles the given configuration's ALPN
    /// with `alpn` by that constructor's rule instead. The server's runtime crate builds its
    /// acceptor from the result.
    pub fn load(&self, alpn: &[&[u8]]) -> Result<Arc<ServerConfig>, TlsError> {
        if let Source::Config(given) = &self.source {
            return reconcile_alpn(given, alpn);
        }
        let (certs, key) = match &self.source {
            Source::PemFiles { cert, key } => (
                certificates(&read(cert)?).map_err(|cause| TlsError::Certificate { path: Some(cert.clone()), cause })?,
                private_key(&read(key)?).map_err(|cause| TlsError::PrivateKey { path: Some(key.clone()), cause })?,
            ),
            Source::Pem { cert, key } => (
                certificates(cert).map_err(|cause| TlsError::Certificate { path: None, cause })?,
                private_key(key).map_err(|cause| TlsError::PrivateKey { path: None, cause })?,
            ),
            Source::Config(_) => unreachable!("a given configuration returned above"),
        };
        // An explicit provider rather than the process default, which panics when the build
        // enables both rustls backends and nothing installed one.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // rustls would refuse these bytes inside `with_single_cert` with a free-text error; asking
        // the provider first keeps the cause a variant of ours.
        if provider.key_provider.load_private_key(key.clone_key()).is_err() {
            return Err(TlsError::PrivateKey { path: key_path(&self.source), cause: KeyCause::Unrecognised });
        }
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
        Source::Pem { .. } | Source::Config(_) => None,
    }
}

fn reconcile_alpn(given: &Arc<ServerConfig>, required: &[&[u8]]) -> Result<Arc<ServerConfig>, TlsError> {
    if required.is_empty() {
        return Ok(Arc::clone(given));
    }
    let named = &given.alpn_protocols;
    let missing: Vec<String> =
        required.iter().filter(|protocol| !named.iter().any(|n| n == **protocol)).map(|protocol| String::from_utf8_lossy(protocol).into_owned()).collect();
    if !named.is_empty() && !missing.is_empty() {
        return Err(TlsError::Alpn {
            required: required.iter().map(|protocol| String::from_utf8_lossy(protocol).into_owned()).collect(),
            given: named.iter().map(|protocol| String::from_utf8_lossy(protocol).into_owned()).collect(),
            missing,
        });
    }
    let protocols: Vec<Vec<u8>> = required.iter().map(|protocol| protocol.to_vec()).collect();
    if &protocols == named {
        return Ok(Arc::clone(given));
    }
    let mut config = ServerConfig::clone(given);
    config.alpn_protocols = protocols;
    Ok(Arc::new(config))
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
    /// The section parses as PEM and holds bytes that are no RSA, ECDSA or EdDSA private key the
    /// TLS backend supports.
    Unrecognised,
    /// The key parses and is not the certificate's.
    DoesNotMatchCertificate,
}

impl fmt::Display for KeyCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyCause::NoneFound => f.write_str("no private key section found"),
            KeyCause::Unparseable { reason } => write!(f, "does not parse: {reason}"),
            KeyCause::Unrecognised => f.write_str("is not an RSA, ECDSA or EdDSA private key"),
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
    /// The key PEM holds no private key, one that does not parse or is not a supported key, or one
    /// that is not the certificate's.
    PrivateKey { path: Option<PathBuf>, cause: KeyCause },
    /// A configuration given to [`Tls::from_config`] names ALPN protocols and omits one the
    /// transport needs. `required` is what the transport needs, `given` what the configuration
    /// names and `missing` the part of `required` it lacks.
    Alpn { required: Vec<String>, given: Vec<String>, missing: Vec<String> },
    /// rustls refused the configuration for another reason.
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
            TlsError::Alpn { required, given, missing } => write!(
                f,
                "bad TLS configuration: its ALPN protocols {given:?} lack {missing:?}, which this server needs (it offers {required:?}); leave `alpn_protocols` empty or name them all"
            ),
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
