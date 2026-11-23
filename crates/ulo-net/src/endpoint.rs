use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use crate::activation::{Activation, ActivationError};
use crate::host::HostPort;

/// Where a server listens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// `"0.0.0.0:8080"`; port 0 lets the OS choose, and `Server::bound` reports its choice.
    Addr(SocketAddr),
    /// A socket from `LISTEN_FDS` (systemd socket activation, or `ulo dev` holding it across
    /// restarts), accepted only when `LISTEN_PID` is this process's ID.
    Inherited(ListenerName),
}

/// Which inherited socket: by its `LISTEN_FDNAMES` name, or by its position among `LISTEN_FDS`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ListenerName {
    Named(Cow<'static, str>),
    Index(usize),
}

impl Endpoint {
    /// `"0.0.0.0:0"`, `"[::1]:8080"`: a socket address, no name resolution.
    pub fn parse(text: &str) -> Result<Endpoint, EndpointError> {
        text.parse::<SocketAddr>()
            .map(Endpoint::Addr)
            .map_err(|_| EndpointError { text: text.to_owned(), expected: Expected::Address })
    }

    /// The inherited socket `LISTEN_FDNAMES` names `name`.
    pub fn inherited(name: impl Into<Cow<'static, str>>) -> Endpoint {
        Endpoint::Inherited(ListenerName::Named(name.into()))
    }

    /// The inherited socket at `index` among `LISTEN_FDS`, from 0 (file descriptor 3).
    pub fn inherited_index(index: usize) -> Endpoint {
        Endpoint::Inherited(ListenerName::Index(index))
    }
}

impl FromStr for Endpoint {
    type Err = EndpointError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Endpoint::parse(text)
    }
}

impl From<SocketAddr> for Endpoint {
    fn from(addr: SocketAddr) -> Self {
        Endpoint::Addr(addr)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Addr(addr) => fmt::Display::fmt(addr, f),
            Endpoint::Inherited(ListenerName::Named(name)) => write!(f, "inherited socket `{name}`"),
            Endpoint::Inherited(ListenerName::Index(index)) => write!(f, "inherited socket #{index}"),
        }
    }
}

/// What an endpoint names before any lookup: an [`Endpoint`], or a host name and port, which the
/// TCP and UDP links resolve through the system resolver ([`HostPort::resolve`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Endpoint(Endpoint),
    Host(HostPort),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Endpoint(endpoint) => fmt::Display::fmt(endpoint, f),
            Target::Host(host) => fmt::Display::fmt(host, f),
        }
    }
}

/// Text that is not a socket address, or, where a host name is accepted, not a host name and port
/// either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointError {
    text: String,
    expected: Expected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Address,
    AddressOrHost,
    /// A host name and port whose name has an empty label.
    EmptyLabel,
}

impl EndpointError {
    pub(crate) fn address_or_host(text: &str) -> EndpointError {
        EndpointError { text: text.to_owned(), expected: Expected::AddressOrHost }
    }

    pub(crate) fn empty_label(text: &str) -> EndpointError {
        EndpointError { text: text.to_owned(), expected: Expected::EmptyLabel }
    }
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = Escaped(&self.text);
        match self.expected {
            Expected::Address => write!(f, "`{text}` is not a socket address such as `0.0.0.0:8080`"),
            Expected::AddressOrHost => {
                write!(f, "`{text}` is neither a socket address such as `0.0.0.0:8080` nor a host name and port such as `billing:7000`")
            }
            Expected::EmptyLabel => write!(
                f,
                "`{text}` is neither a socket address such as `0.0.0.0:8080` nor a host name and port such as `billing:7000`: \
                 its name has an empty label, a dot beginning it or two dots together; a single trailing dot is accepted"
            ),
        }
    }
}

/// Text as written with each control character and each format character escaped, `\u{0}` for a
/// NUL and `\u{202e}` for a right-to-left override, so a refusal prints on one line, in the order
/// written, and holds nothing a terminal or a log reader acts on.
pub(crate) struct Escaped<'a>(pub(crate) &'a str);

impl fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            if c.is_control() || is_format(c) {
                write!(f, "{}", c.escape_default())?;
            } else {
                fmt::Write::write_char(f, c)?;
            }
        }
        Ok(())
    }
}

/// Unicode general category Cf as of Unicode 15.1, which `char` has no test for: the bidi
/// embeddings, overrides and isolates, zero-width spaces and joiners, the byte-order mark, and the
/// rest of the format characters.
const FORMAT: &[(char, char)] = &[
    ('\u{ad}', '\u{ad}'),
    ('\u{600}', '\u{605}'),
    ('\u{61c}', '\u{61c}'),
    ('\u{6dd}', '\u{6dd}'),
    ('\u{70f}', '\u{70f}'),
    ('\u{890}', '\u{891}'),
    ('\u{8e2}', '\u{8e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{202a}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

pub(crate) fn is_format(c: char) -> bool {
    FORMAT.iter().any(|&(first, last)| (first..=last).contains(&c))
}

impl Error for EndpointError {}

/// What a server builder takes: an [`Endpoint`], a `SocketAddr`, or text, which the server parses
/// in `prepare` so a bad address is a `StartupError::Configure` beside every other configuration
/// error rather than a failure in `main`.
#[derive(Clone, Debug)]
pub struct EndpointSpec(Spec);

#[derive(Clone, Debug)]
enum Spec {
    Parsed(Endpoint),
    Text(String),
}

impl EndpointSpec {
    /// The endpoint a server listens on, for its `prepare`.
    ///
    /// Under `ULO_DEV=1`, which `ulo dev` sets in every child it starts, an [`Endpoint::Addr`]
    /// resolves to the inherited socket named after that address when this process holds one, so
    /// an application written `Server::new("0.0.0.0:8080")` adopts the socket `ulo dev --listen
    /// 0.0.0.0:8080` holds across restarts and binds the address itself in production. The
    /// address is compared as parsed, so `"0.0.0.0:8080"`, the `SocketAddr` and
    /// `Endpoint::Addr(..)` match alike. Without the variable, with no socket named after the
    /// address, or with inherited sockets this process cannot adopt (a `LISTEN_PID` naming another
    /// process among them), the address resolves as written. An [`Endpoint::Inherited`] resolves
    /// as written in either case.
    pub fn resolve(&self) -> Result<Endpoint, EndpointError> {
        let endpoint = self.resolve_as_written()?;
        Ok(match endpoint {
            Endpoint::Addr(addr) if dev_mode() => held_by_dev(addr).unwrap_or(endpoint),
            endpoint => endpoint,
        })
    }

    /// The endpoint as written, without the `ULO_DEV` lookup [`resolve`](Self::resolve) makes:
    /// for a client, which connects to the address, and for a socket the activation cannot carry,
    /// such as a UDP link's.
    pub fn resolve_as_written(&self) -> Result<Endpoint, EndpointError> {
        match &self.0 {
            Spec::Parsed(endpoint) => Ok(endpoint.clone()),
            Spec::Text(text) => Endpoint::parse(text),
        }
    }

    /// What the endpoint names as written, a host name accepted: text that is a socket address is
    /// an [`Endpoint::Addr`], as [`resolve_as_written`](Self::resolve_as_written) reads it, and
    /// other text a [`HostPort`]. Nothing is looked up, and no `ULO_DEV` lookup is made. For a
    /// link that resolves host names, the TCP and UDP links; a server listening on a host name
    /// resolves it once, and [`resolve`](Self::resolve) on the address it picks makes the
    /// `ULO_DEV` lookup.
    pub fn target(&self) -> Result<Target, EndpointError> {
        match &self.0 {
            Spec::Parsed(endpoint) => Ok(Target::Endpoint(endpoint.clone())),
            Spec::Text(text) => match Endpoint::parse(text) {
                Ok(endpoint) => Ok(Target::Endpoint(endpoint)),
                Err(_) => HostPort::parse(text).map(Target::Host),
            },
        }
    }
}

const DEV_VAR: &str = "ULO_DEV";

fn dev_mode() -> bool {
    std::env::var_os(DEV_VAR).is_some_and(|value| value == "1")
}

/// The inherited socket `ulo dev --listen` named after `addr`, when this process holds it.
fn held_by_dev(addr: SocketAddr) -> Option<Endpoint> {
    let activation = match Activation::get() {
        Ok(activation) => activation,
        // A process the application spawns inherits `ULO_DEV` beside a `LISTEN_PID` that is not
        // its own; off Unix nothing is ever inherited.
        Err(error @ (ActivationError::PidMismatch { .. } | ActivationError::Unsupported)) => {
            tracing::debug!("{DEV_VAR}=1, and no inherited socket is this process's ({error}); {addr} binds as written");
            return None;
        }
        Err(error) => {
            tracing::warn!("{DEV_VAR}=1, and the inherited sockets cannot be adopted ({error}); {addr} binds as written");
            return None;
        }
    };
    let name = ListenerName::Named(Cow::Owned(dev_socket_name(addr)));
    activation.contains(&name).then_some(Endpoint::Inherited(name))
}

/// The `LISTEN_FDNAMES` name `ulo dev --listen` gives the socket it holds on `addr`: the address's
/// text with `%` written `%25` and `:` written `%3A`. `LISTEN_FDNAMES` separates names with `:`,
/// which every socket address contains. `ulo-cli`'s `commands/dev.rs` writes the same encoding.
fn dev_socket_name(addr: SocketAddr) -> String {
    let text = addr.to_string();
    let mut name = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '%' => name.push_str("%25"),
            ':' => name.push_str("%3A"),
            c => name.push(c),
        }
    }
    name
}

impl From<Endpoint> for EndpointSpec {
    fn from(endpoint: Endpoint) -> Self {
        EndpointSpec(Spec::Parsed(endpoint))
    }
}

impl From<SocketAddr> for EndpointSpec {
    fn from(addr: SocketAddr) -> Self {
        EndpointSpec(Spec::Parsed(Endpoint::Addr(addr)))
    }
}

impl From<&str> for EndpointSpec {
    fn from(text: &str) -> Self {
        EndpointSpec(Spec::Text(text.to_owned()))
    }
}

impl From<String> for EndpointSpec {
    fn from(text: String) -> Self {
        EndpointSpec(Spec::Text(text))
    }
}
