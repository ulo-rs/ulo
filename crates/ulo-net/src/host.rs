use std::error::Error;
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use ulo::{Redacted, Runtime, TaskEnd, spawn_blocking_with};

use crate::endpoint::{EndpointError, Escaped};

/// A host name and port, `billing:7000`, which a link resolves to socket addresses through the
/// system resolver.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostPort {
    host: String,
    port: u16,
}

impl HostPort {
    /// `billing:7000`, `billing.default.svc.cluster.local.:7000`: a name, then a decimal port after
    /// the last `:`. The name is kept as written, a trailing dot included, since `/etc/hosts` and
    /// search lists read `billing.` and `billing` differently. Refused: no port, a port above
    /// 65535, an empty name, a name holding a `:`, a bracket, whitespace, a control character or
    /// one of `/@?#`, none of which a host name holds, and a name with an empty label other than
    /// the one a trailing dot ends, `.`, `..`, `a..b` or `.a`, which no lookup answers. A label
    /// over 63 bytes, a name over 253 and an underscore are left to the resolver, since
    /// `/etc/hosts` can hold such names. An IP address and port is a socket address, which
    /// `Endpoint::parse` reads; this one does not tell the two apart.
    pub fn parse(text: &str) -> Result<HostPort, EndpointError> {
        let refused = || EndpointError::address_or_host(text);
        let (host, port) = text.rsplit_once(':').ok_or_else(refused)?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refused());
        }
        let port = port.parse::<u16>().map_err(|_| refused())?;
        let foreign = |c: char| c.is_whitespace() || c.is_control() || matches!(c, ':' | '[' | ']' | '/' | '@' | '?' | '#');
        if host.is_empty() || host.contains(foreign) {
            return Err(refused());
        }
        if host.strip_suffix('.').unwrap_or(host).split('.').any(str::is_empty) {
            return Err(EndpointError::empty_label(text));
        }
        Ok(HostPort { host: host.to_owned(), port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The addresses the system resolver answers for the name, in its order, with the port. The
    /// lookup is `ToSocketAddrs`, so `/etc/hosts`, nsswitch and the search list apply as they do
    /// for any program on the host, and it runs as `runtime`'s blocking work
    /// ([`Spawn::spawn_blocking`](ulo::Spawn::spawn_blocking)), so no thread polling a task waits
    /// on it. Each call looks the name up again: nothing is cached.
    pub async fn resolve(&self, runtime: &dyn Runtime) -> Result<Vec<SocketAddr>, ResolveError> {
        Lookup::system().resolve(self, runtime).await
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// How a host name becomes addresses: [`Lookup::system`], `ToSocketAddrs`, or a function a test
/// plants to answer addresses of its choosing, call by call. Either runs as the runtime's blocking
/// work.
#[derive(Clone)]
pub struct Lookup(Option<Arc<LookupFn>>);

type LookupFn = dyn Fn(&HostPort) -> io::Result<Vec<SocketAddr>> + Send + Sync;

impl Lookup {
    /// The system resolver, through `ToSocketAddrs`.
    pub fn system() -> Lookup {
        Lookup(None)
    }

    /// `lookup` in place of the system resolver.
    pub fn from_fn(lookup: impl Fn(&HostPort) -> io::Result<Vec<SocketAddr>> + Send + Sync + 'static) -> Lookup {
        Lookup(Some(Arc::new(lookup)))
    }

    /// The addresses this lookup answers for `host`, run as `runtime`'s blocking work.
    pub async fn resolve(&self, host: &HostPort, runtime: &dyn Runtime) -> Result<Vec<SocketAddr>, ResolveError> {
        let (lookup, asked) = (self.0.clone(), host.clone());
        let answered = spawn_blocking_with(runtime, move || match lookup {
            Some(lookup) => lookup(&asked),
            None => (asked.host.as_str(), asked.port).to_socket_addrs().map(Iterator::collect),
        })
        .await;
        let cause = match answered {
            Ok(Ok(addrs)) if !addrs.is_empty() => return Ok(addrs),
            Ok(Ok(_)) => ResolveCause::NoAddress,
            Ok(Err(error)) => ResolveCause::Lookup(error),
            Err(TaskEnd::Panicked(message)) => ResolveCause::Panicked(message),
            Err(_) => ResolveCause::Refused,
        };
        Err(ResolveError { host: host.clone(), cause })
    }
}

impl Default for Lookup {
    fn default() -> Lookup {
        Lookup::system()
    }
}

impl fmt::Debug for Lookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "Lookup::from_fn(..)" } else { "Lookup::system()" })
    }
}

/// A host name that resolved to no address, and why.
#[derive(Debug)]
pub struct ResolveError {
    host: HostPort,
    cause: ResolveCause,
}

/// Why a host name resolved to no address.
#[derive(Debug)]
#[non_exhaustive]
pub enum ResolveCause {
    /// The resolver failed: no such name, no answer, or no resolver reachable. Its error is the
    /// `ResolveError`'s source.
    Lookup(io::Error),
    /// The resolver answered, with no address.
    NoAddress,
    /// The runtime ran no lookup: it has shut down.
    Refused,
    /// The lookup panicked, its message redacted.
    Panicked(Arc<Redacted>),
}

impl ResolveError {
    pub fn host(&self) -> &HostPort {
        &self.host
    }

    pub fn cause(&self) -> &ResolveCause {
        &self.cause
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let written = self.host.to_string();
        let host = Escaped(&written);
        match &self.cause {
            ResolveCause::Lookup(error) => write!(f, "`{host}` resolved to no address: {error}"),
            ResolveCause::NoAddress => write!(f, "`{host}` resolved to no address: the resolver answered none"),
            ResolveCause::Refused => write!(f, "`{host}` was not resolved: the runtime has shut down and ran no lookup"),
            ResolveCause::Panicked(message) => write!(f, "`{host}` was not resolved: the lookup panicked: {message}"),
        }
    }
}

impl Error for ResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.cause {
            ResolveCause::Lookup(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_character_in_a_name_that_resolved_to_nothing_is_escaped_in_the_message() {
        let error = ResolveError { host: HostPort::parse("bil\u{202e}ling:7000").expect("a name with a format character parses"), cause: ResolveCause::NoAddress };
        let text = error.to_string();
        assert!(!text.contains('\u{202e}'), "the message prints the override raw: {text}");
        assert!(text.contains("bil\\u{202e}ling:7000"), "{text}");
    }

    #[test]
    fn a_host_name_and_port_parse_and_everything_else_is_refused() {
        for (text, host, port) in [("billing:7000", "billing", 7000), ("billing.default.svc.cluster.local.:7000", "billing.default.svc.cluster.local.", 7000), ("Billing_1:0", "Billing_1", 0)] {
            let parsed = HostPort::parse(text).unwrap_or_else(|error| panic!("`{text}` was refused: {error}"));
            assert_eq!((parsed.host(), parsed.port()), (host, port), "`{text}`");
            assert_eq!(parsed.to_string(), text, "`{text}` displayed");
        }
        for text in ["billing", "billing:", ":7000", "billing:70000", "billing:+70", "billing:7a", "::1:7000", "[::1]:x", "bil ling:7000", "user@billing:7000", "billing/x:7000"] {
            assert!(HostPort::parse(text).is_err(), "`{text}` parsed as a host name and port");
        }
    }

    /// An empty label is refused at parse, where a link is set up, except the one a trailing dot
    /// ends; a long label and an underscore are left to the resolver.
    #[test]
    fn a_name_with_an_empty_label_is_refused_and_one_the_resolver_decides_is_not() {
        for text in [".:7000", "..:7000", "a..b:7000", ".a:7000", "billing..:7000"] {
            match HostPort::parse(text) {
                Ok(parsed) => panic!("`{text}` parsed as {parsed:?}"),
                Err(refused) => assert!(refused.to_string().contains("its name has an empty label"), "`{text}`: {refused}"),
            }
        }
        let long = format!("{}.example:7000", "a".repeat(64));
        for text in ["billing.:7000", "a_b:7000", long.as_str()] {
            let parsed = HostPort::parse(text).unwrap_or_else(|error| panic!("`{text}` was refused: {error}"));
            assert_eq!(parsed.to_string(), text, "`{text}` kept as written");
        }
    }

    /// A control character in the text is refused, and the refusal prints it escaped, so the
    /// message holds no raw NUL.
    #[test]
    fn a_control_character_is_refused_and_escaped_in_the_refusal() {
        for (text, shown) in [("bil\0ling:7000", "bil\\u{0}ling:7000"), ("billing\n:7000", "billing\\n:7000"), ("\u{1b}[2J:7000", "\\u{1b}[2J:7000")] {
            let refused = HostPort::parse(text).expect_err("a name holding a control character");
            let message = refused.to_string();
            assert!(!message.contains(|c: char| c.is_control()), "`{text:?}`: the refusal holds a control character: {message:?}");
            assert!(message.contains(&format!("`{shown}`")), "`{text:?}`: the refusal does not show the text escaped: {message:?}");
        }
    }

    /// A format character in refused text, a bidi override or isolate, a zero-width space or a
    /// byte-order mark, is printed escaped, so the refusal reads in the order the text was written.
    #[test]
    fn a_format_character_in_the_refusal_is_escaped() {
        for (text, shown) in [
            ("bil\u{202e}ling", "bil\\u{202e}ling"),
            ("bil\u{200b}ling", "bil\\u{200b}ling"),
            ("\u{feff}billing", "\\u{feff}billing"),
            ("bil\u{2066}ling", "bil\\u{2066}ling"),
        ] {
            let message = HostPort::parse(text).expect_err("text with no port").to_string();
            assert!(message.contains(&format!("`{shown}`")), "{text:?}: the refusal does not show the text escaped: {message:?}");
            assert!(!message.contains(|c: char| crate::endpoint::is_format(c)), "{text:?}: the refusal holds a format character: {message:?}");
        }
    }
}
