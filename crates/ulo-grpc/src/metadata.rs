//! One reply metadata entry, checked where it is built: its key against the gRPC over HTTP/2 key
//! grammar and the names the protocol reserves, and its value's form against the key's `-bin`
//! suffix (transports DESIGN §6.2).

use std::error::Error;
use std::fmt;

use tonic::metadata::{Ascii, Binary, MetadataKey, MetadataMap, MetadataValue};

/// The names a reply's metadata never sets besides the `grpc-` family: the three the reply writes,
/// and the connection-specific names HTTP/2 forbids in a message (RFC 9113 §8.2.2), which hyper
/// strips from a reply's headers and h2 refuses in its trailers by resetting the stream.
const HTTP_OWNED: [&str; 8] = ["te", "content-type", "content-length", "connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade"];

/// Whether reply metadata may not carry `name`, a lowercase header name. Every name beginning
/// `grpc-` is the protocol's: gRPC over HTTP/2 reserves the prefix, and the reply and its trailers
/// write the ones in use, `grpc-status`, `grpc-message` and `grpc-status-details-bin` among them.
pub(crate) fn reserved(name: &str) -> bool {
    name.starts_with("grpc-") || HTTP_OWNED.contains(&name)
}

/// One metadata entry for a reply's headers or trailers: a key and an ASCII or binary value, the
/// form matching the key. A key ending `-bin` carries bytes, sent base64-encoded and read back as
/// the bytes; any other key carries printable ASCII. [`Response::metadata_entry`] puts one on the
/// headers and [`Response::trailer_entry`] on the trailers.
///
/// Each constructor refuses, as a [`MetadataError`]:
/// - a key off gRPC's grammar, which is one or more of `0`-`9`, `a`-`z`, `_`, `-` and `.`, an
///   uppercase ASCII letter being read as its lowercase;
/// - a key gRPC over HTTP/2 reserves, any beginning `grpc-`, and `te`, `content-type` and
///   `content-length`, which the reply writes, and `connection`, `keep-alive`,
///   `proxy-connection`, `transfer-encoding` and `upgrade`, which HTTP/2 forbids in a message;
/// - an ASCII value under a key ending `-bin`, and a binary value under any other key;
/// - an ASCII value holding a byte outside `0x20` to `0x7E`.
///
/// ```
/// use ulo_grpc::{MetadataEntry, MetadataError, Response};
///
/// let digest = MetadataEntry::binary("x-digest-bin", vec![0x00, 0xff, 0x10])?;
/// let rows = MetadataEntry::ascii("x-rows", "12")?;
/// let reply = Response::new(()).trailer_entry(digest).trailer_entry(rows);
/// # let _ = reply;
///
/// assert!(matches!(MetadataEntry::ascii("x-digest-bin", "AAEC"), Err(MetadataError::AsciiUnderBinaryKey { .. })));
/// assert!(matches!(MetadataEntry::ascii("grpc-status", "0"), Err(MetadataError::Reserved { .. })));
/// # Ok::<(), MetadataError>(())
/// ```
///
/// [`Response::metadata_entry`]: crate::Response::metadata_entry
/// [`Response::trailer_entry`]: crate::Response::trailer_entry
#[derive(Clone, Debug)]
pub struct MetadataEntry {
    form: Form,
}

#[derive(Clone, Debug)]
enum Form {
    Ascii(MetadataKey<Ascii>, MetadataValue<Ascii>),
    Binary(MetadataKey<Binary>, MetadataValue<Binary>),
}

impl MetadataEntry {
    /// An ASCII entry: `key` must not end `-bin`, and `value` is printable ASCII.
    pub fn ascii(key: &str, value: &str) -> Result<MetadataEntry, MetadataError> {
        let name = checked_key(key)?;
        if name.ends_with("-bin") {
            return Err(MetadataError::AsciiUnderBinaryKey { key: name });
        }
        if !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
            return Err(MetadataError::InvalidAscii { key: name });
        }
        let key = MetadataKey::<Ascii>::from_bytes(name.as_bytes()).map_err(|_| MetadataError::InvalidKey { key: name.clone() })?;
        let value = MetadataValue::<Ascii>::try_from(value).map_err(|_| MetadataError::InvalidAscii { key: name })?;
        Ok(MetadataEntry { form: Form::Ascii(key, value) })
    }

    /// A binary entry: `key` must end `-bin`, and `value` is any bytes.
    pub fn binary(key: &str, value: impl Into<Vec<u8>>) -> Result<MetadataEntry, MetadataError> {
        let name = checked_key(key)?;
        if !name.ends_with("-bin") {
            return Err(MetadataError::BinaryUnderAsciiKey { key: name });
        }
        let key = MetadataKey::<Binary>::from_bytes(name.as_bytes()).map_err(|_| MetadataError::InvalidKey { key: name })?;
        let value: Vec<u8> = value.into();
        Ok(MetadataEntry { form: Form::Binary(key, MetadataValue::from_bytes(&value)) })
    }

    /// The key, lowercase.
    pub fn key(&self) -> &str {
        match &self.form {
            Form::Ascii(key, _) => key.as_str(),
            Form::Binary(key, _) => key.as_str(),
        }
    }

    /// Appends the entry to `map`, beside any value its key already holds.
    pub(crate) fn append_to(self, map: &mut MetadataMap) {
        match self.form {
            Form::Ascii(key, value) => {
                map.append(key, value);
            }
            Form::Binary(key, value) => {
                map.append_bin(key, value);
            }
        }
    }
}

/// `key` lowercased, when it is on gRPC's key grammar and not reserved.
fn checked_key(key: &str) -> Result<String, MetadataError> {
    let name = key.to_ascii_lowercase();
    let on_grammar = !name.is_empty() && name.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'z' | b'_' | b'-' | b'.'));
    if !on_grammar {
        return Err(MetadataError::InvalidKey { key: key.to_owned() });
    }
    if reserved(&name) {
        return Err(MetadataError::Reserved { key: name });
    }
    Ok(name)
}

/// Why a [`MetadataEntry`] was refused. Each variant names the key, lowercased once it is on the
/// grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetadataError {
    /// The key is empty or holds a character outside `0`-`9`, `a`-`z`, `_`, `-` and `.`.
    InvalidKey { key: String },
    /// The key begins `grpc-`, is `te`, `content-type` or `content-length`, or is a
    /// connection-specific name HTTP/2 forbids: `connection`, `keep-alive`, `proxy-connection`,
    /// `transfer-encoding` or `upgrade`.
    Reserved { key: String },
    /// An ASCII value under a key ending `-bin`, which carries bytes.
    AsciiUnderBinaryKey { key: String },
    /// A binary value under a key not ending `-bin`, which carries ASCII.
    BinaryUnderAsciiKey { key: String },
    /// An ASCII value holding a byte outside `0x20` to `0x7E`.
    InvalidAscii { key: String },
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataError::InvalidKey { key } => {
                write!(f, "the metadata key {key:?} is not one or more of `0`-`9`, `a`-`z`, `_`, `-` and `.`")
            }
            MetadataError::Reserved { key } => write!(
                f,
                "the metadata key `{key}` is reserved: gRPC over HTTP/2 reserves every name beginning `grpc-`, the reply writes `te`, `content-type` and `content-length`, and HTTP/2 forbids the connection-specific `connection`, `keep-alive`, `proxy-connection`, `transfer-encoding` and `upgrade`"
            ),
            MetadataError::AsciiUnderBinaryKey { key } => {
                write!(f, "the metadata key `{key}` ends `-bin` and carries bytes: build the entry with `MetadataEntry::binary`")
            }
            MetadataError::BinaryUnderAsciiKey { key } => write!(
                f,
                "the metadata key `{key}` does not end `-bin` and carries ASCII: build the entry with `MetadataEntry::ascii`, or end the key `-bin`"
            ),
            MetadataError::InvalidAscii { key } => {
                write!(f, "the value for the metadata key `{key}` holds a byte outside printable ASCII, `0x20` to `0x7E`")
            }
        }
    }
}

impl Error for MetadataError {}

/// `map` without the names [`reserved`] covers, each dropped logged at `warn`: what a handler's
/// `tonic::Status` carries in its metadata before the status is written.
pub(crate) fn screened(map: &mut MetadataMap) {
    let headers = std::mem::take(map).into_headers();
    if !headers.keys().any(|name| reserved(name.as_str())) {
        *map = MetadataMap::from_headers(headers);
        return;
    }
    let mut kept = http::HeaderMap::new();
    for (name, value) in &headers {
        if reserved(name.as_str()) {
            tracing::warn!(key = name.as_str(), "status metadata dropped: gRPC over HTTP/2 reserves the name");
        } else {
            kept.append(name.clone(), value.clone());
        }
    }
    *map = MetadataMap::from_headers(kept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ascii_entry_refuses_a_binary_key_and_a_binary_entry_an_ascii_key() {
        assert_eq!(MetadataEntry::ascii("x-a-bin", "v").err(), Some(MetadataError::AsciiUnderBinaryKey { key: "x-a-bin".into() }));
        assert_eq!(MetadataEntry::binary("x-a", [1u8].as_slice()).err(), Some(MetadataError::BinaryUnderAsciiKey { key: "x-a".into() }));
        assert!(MetadataEntry::ascii("x-a", "v").is_ok());
        assert!(MetadataEntry::binary("x-a-bin", [1u8].as_slice()).is_ok());
    }

    /// The suffix is read after lowercasing, so `-BIN` is a binary key.
    #[test]
    fn the_suffix_is_read_lowercased() {
        assert_eq!(MetadataEntry::ascii("X-A-BIN", "v").err(), Some(MetadataError::AsciiUnderBinaryKey { key: "x-a-bin".into() }));
        let entry = MetadataEntry::binary("X-A-BIN", [1u8].as_slice()).expect("a binary key");
        assert_eq!(entry.key(), "x-a-bin");
    }

    #[test]
    fn every_grpc_prefixed_name_and_the_http_owned_ones_are_refused_from_both_forms() {
        for key in ["grpc-status", "grpc-message", "grpc-status-details-bin", "grpc-encoding", "grpc-x", "GRPC-Y", "te", "content-type", "content-length", "connection", "keep-alive", "proxy-connection", "transfer-encoding", "upgrade"] {
            let ascii = MetadataEntry::ascii(key, "v").err();
            let binary = MetadataEntry::binary(key, [1u8].as_slice()).err();
            for refused in [ascii, binary] {
                assert!(matches!(refused, Some(MetadataError::Reserved { .. })), "{key}: {refused:?}");
            }
        }
        for kept in ["x-grpc-y", "grpcish", "grpc"] {
            assert!(MetadataEntry::ascii(kept, "v").is_ok(), "{kept} is not under the prefix");
        }
    }

    /// gRPC's key grammar is narrower than HTTP's token: `!` is a valid header name character.
    #[test]
    fn a_key_off_grpc_s_grammar_is_refused() {
        for key in ["", "x a", "x!", "x/y", "é", "x:y"] {
            assert_eq!(MetadataEntry::ascii(key, "v").err(), Some(MetadataError::InvalidKey { key: key.into() }), "{key:?}");
        }
        assert!(MetadataEntry::ascii("a_b-c.d9", "v").is_ok());
    }

    #[test]
    fn an_ascii_value_outside_printable_ascii_is_refused() {
        for value in ["a\nb", "tab\there", "\u{7f}", "café"] {
            assert_eq!(MetadataEntry::ascii("x-a", value).err(), Some(MetadataError::InvalidAscii { key: "x-a".into() }), "{value:?}");
        }
        assert!(MetadataEntry::ascii("x-a", " !~").is_ok());
        assert!(MetadataEntry::ascii("x-a", "").is_ok());
    }

    #[test]
    fn a_binary_value_keeps_every_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        let mut map = MetadataMap::new();
        MetadataEntry::binary("x-all-bin", bytes.clone()).expect("a binary key").append_to(&mut map);
        let value = map.get_bin("x-all-bin").expect("appended").to_bytes().expect("base64 the entry wrote");
        assert_eq!(value.as_ref(), bytes.as_slice());
    }

    #[test]
    fn the_status_screen_drops_reserved_names_and_keeps_the_rest() {
        let mut map = MetadataMap::new();
        map.insert("grpc-x", "1".parse().expect("ascii"));
        map.insert_bin("grpc-status-details-bin", MetadataValue::from_bytes(b"forged"));
        map.insert("x-kept", "1".parse().expect("ascii"));
        map.insert("te", "trailers".parse().expect("ascii"));
        screened(&mut map);
        assert!(map.get("grpc-x").is_none() && map.get_bin("grpc-status-details-bin").is_none() && map.get("te").is_none(), "{map:?}");
        assert_eq!(map.get("x-kept").and_then(|value| value.to_str().ok()), Some("1"));
    }
}
