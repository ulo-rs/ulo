//! The client's address behind reverse proxies (transports DESIGN §3.6): `behind_proxy`, the
//! forwarding header a server's proxies write and the CIDR ranges it trusts to write it, and the
//! walk over that header that [`HttpCx::client_ip`](crate::HttpCx::client_ip) runs.

use std::convert::Infallible;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use http::header::{FORWARDED, HeaderMap, HeaderValue};

/// The `X-Forwarded-For` header, which `http` names no constant for.
const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// The `X-Forwarded-Host` header, read with `X-Forwarded-For`.
const X_FORWARDED_HOST: &str = "x-forwarded-host";

/// The forwarding header a server's reverse proxies write, named with the ranges they connect
/// from in [`Server::behind_proxy`](crate::Server::behind_proxy). `client_ip()` reads that
/// header alone; a request carrying the other is read as carrying neither.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyHeader {
    /// RFC 7239 `Forwarded`, each hop the `for` parameter of one element, and the host a
    /// WebSocket handshake addressed the `host` parameter of the rightmost one. nginx has no
    /// variable that writes it; this configuration replaces whatever the client sent with the
    /// address nginx saw and the `Host` the client sent, the two `map`s in the `http` block, the
    /// first bracketing and quoting an IPv6 address as RFC 7239 §6 requires and writing a
    /// unix-socket client as `unknown`:
    ///
    /// ```nginx
    /// map $remote_addr $forwarded_for {
    ///     ~^unix:  "for=unknown";
    ///     ~:       "for=\"[$remote_addr]\"";
    ///     default  "for=$remote_addr";
    /// }
    /// map $http_host $forwarded_host {
    ///     "~^[A-Za-z0-9.:\[\]-]+$"  $http_host;
    ///     default                  "";
    /// }
    /// proxy_set_header Forwarded "$forwarded_for;host=\"$forwarded_host\"";
    /// ```
    ///
    /// The second `map` admits only the characters of a host and port. nginx accepts `"`, `,`
    /// and `;` in a `Host`, so a `Host` written into the quoted string unchecked lets a client
    /// close it and append an element whose `for` names any address, which `client_ip()` then
    /// answers. Any other `Host` is written as `host=""`, which compares equal to no origin.
    /// `$http_host` keeps the port the browser addressed; `$host` drops it, and an `Origin` on a
    /// port other than its scheme's default then matches nothing.
    Forwarded,
    /// `X-Forwarded-For`, a comma-separated list of addresses, with `X-Forwarded-Host` the host a
    /// WebSocket handshake addressed. nginx appends the address it saw to what the client sent
    /// with `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`, and the walk from the
    /// right never reaches the client's part of the list; `proxy_set_header X-Forwarded-Host
    /// $http_host;` replaces the host with the `Host` the client sent, its port kept.
    XForwardedFor,
}

impl ProxyHeader {
    fn name(self) -> &'static str {
        match self {
            ProxyHeader::Forwarded => FORWARDED.as_str(),
            ProxyHeader::XForwardedFor => X_FORWARDED_FOR,
        }
    }
}

/// `behind_proxy` as a server holds it: the header and the ranges trusted to write it.
#[derive(Clone, Debug)]
pub(crate) struct BehindProxy {
    pub(crate) header: ProxyHeader,
    pub(crate) trusted: Vec<Cidr>,
}

/// A range of IP addresses in CIDR notation, `10.0.0.0/8` or `2001:db8::/32`, as a server's
/// [`behind_proxy`](crate::Server::behind_proxy) lists them.
///
/// Parsed from text by `FromStr` or `TryFrom<&str>`: an address, `/`, and a prefix length of at
/// most 32 for IPv4 and 128 for IPv6; an address alone is the range of that one address, `/32` or
/// `/128`. Bits past the prefix are ignored, so `10.1.2.3/8` is `10.0.0.0/8`. An IPv4-mapped IPv6
/// range, `::ffff:10.0.0.0/104`, is the IPv4 range it maps, `10.0.0.0/8`, and an IPv4-mapped
/// address is matched as the IPv4 address it maps: a dual-stack socket reports an IPv4 peer as
/// `::ffff:a.b.c.d`. An IPv6 range shorter than `/96` holds no IPv4 address, `::/0` included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

/// Why text is not a [`Cidr`]; its `Display` names the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CidrError {
    input: String,
    reason: Reason,
}

const _: () = assert!(size_of::<CidrError>() < 128, "clippy's `result_large_err` fires at 128 bytes");

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reason {
    Address,
    Prefix,
    PrefixTooLong { prefix: String, max: u8 },
}

impl Cidr {
    /// The range of the addresses sharing `addr`'s first `prefix` bits. Refused: a prefix over 32
    /// for an IPv4 address or over 128 for an IPv6 one.
    pub fn new(addr: IpAddr, prefix: u8) -> Result<Cidr, CidrError> {
        Cidr::checked(addr, u32::from(prefix)).map_err(|reason| CidrError { input: format!("{addr}/{prefix}"), reason })
    }

    fn checked(addr: IpAddr, prefix: u32) -> Result<Cidr, Reason> {
        let max = if addr.is_ipv4() { 32 } else { 128 };
        if prefix > u32::from(max) {
            return Err(Reason::PrefixTooLong { prefix: prefix.to_string(), max });
        }
        let prefix = prefix as u8;
        Ok(match addr {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) if prefix >= 96 => Cidr { network: IpAddr::V4(mask_v4(v4, prefix - 96)), prefix: prefix - 96 },
                _ => Cidr { network: IpAddr::V6(mask_v6(v6, prefix)), prefix },
            },
            IpAddr::V4(v4) => Cidr { network: IpAddr::V4(mask_v4(v4, prefix)), prefix },
        })
    }

    /// The range's first address.
    pub fn network(&self) -> IpAddr {
        self.network
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    /// Whether `ip` is in the range, an IPv4-mapped IPv6 address read as the IPv4 address it maps.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => mask_v4(ip, self.prefix) == network,
            (IpAddr::V6(network), IpAddr::V6(ip)) => mask_v6(ip, self.prefix) == network,
            _ => false,
        }
    }
}

fn mask_v4(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    Ipv4Addr::from(u32::from(addr) & mask)
}

fn mask_v6(addr: Ipv6Addr, prefix: u8) -> Ipv6Addr {
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    Ipv6Addr::from(u128::from(addr) & mask)
}

impl FromStr for Cidr {
    type Err = CidrError;

    fn from_str(text: &str) -> Result<Cidr, CidrError> {
        let error = |reason| CidrError { input: text.to_owned(), reason };
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr.parse().map_err(|_| error(Reason::Address))?;
        let prefix = match prefix {
            None => if addr.is_ipv4() { 32 } else { 128 },
            Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => match digits.parse::<u32>() {
                Ok(prefix) => prefix,
                Err(_) => {
                    let max = if addr.is_ipv4() { 32 } else { 128 };
                    return Err(error(Reason::PrefixTooLong { prefix: digits.to_owned(), max }));
                }
            },
            Some(_) => return Err(error(Reason::Prefix)),
        };
        Cidr::checked(addr, prefix).map_err(error)
    }
}

impl TryFrom<&str> for Cidr {
    type Error = CidrError;

    fn try_from(text: &str) -> Result<Cidr, CidrError> {
        text.parse()
    }
}

impl TryFrom<String> for Cidr {
    type Error = CidrError;

    fn try_from(text: String) -> Result<Cidr, CidrError> {
        text.parse()
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

impl CidrError {
    /// The text that was refused.
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for CidrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a CIDR range: ", self.input)?;
        match &self.reason {
            Reason::Address => f.write_str("the part before `/` is not an IPv4 or IPv6 address"),
            Reason::Prefix => f.write_str("the part after `/` is not a prefix length in decimal digits"),
            Reason::PrefixTooLong { prefix, max } => write!(f, "the prefix length {prefix} is over {max}, the address's length in bits"),
        }
    }
}

impl std::error::Error for CidrError {}

/// A `Cidr` passed where text could be: `behind_proxy(header, [cidr])` beside
/// `behind_proxy(header, ["10.0.0.0/8"])`.
impl From<Infallible> for CidrError {
    fn from(never: Infallible) -> CidrError {
        match never {}
    }
}

/// The client's address for a request from `peer` carrying `headers`, `proxy` the server's
/// `behind_proxy`. `None` without a peer address, from which nothing can be trusted.
///
/// Without `proxy`, or from a peer outside its ranges, the peer is the client and no header is
/// read. Behind a trusted peer, the hops of the header `proxy` names, every field line in order,
/// are walked from the right: a trusted hop is passed, and the first untrusted one is the client.
/// When every hop is trusted the leftmost is, and with no hop at all the peer is. A hop that names
/// no address, `unknown`, an obfuscated `_node`, a `Forwarded` element without `for`, or text that
/// does not parse, ends the walk, and the client is the last hop passed, or the peer when none
/// was: what such a hop hides cannot be trusted, and what lies left of it was written by whoever
/// it hides. The other forwarding header is never read.
pub(crate) fn client_ip(peer: Option<SocketAddr>, headers: &HeaderMap, proxy: Option<&BehindProxy>) -> Option<IpAddr> {
    let peer = peer?.ip().to_canonical();
    let Some(BehindProxy { header, trusted }) = proxy else { return Some(peer) };
    let trusts = |ip: IpAddr| trusted.iter().any(|range| range.contains(ip));
    if !trusts(peer) {
        return Some(peer);
    }
    let lines = headers.get_all(header.name()).iter();
    let hops: Vec<Option<IpAddr>> = match header {
        ProxyHeader::Forwarded => lines.flat_map(forwarded_line).collect(),
        ProxyHeader::XForwardedFor => lines.flat_map(xff_line).collect(),
    };
    let mut client = peer;
    for hop in hops.into_iter().rev() {
        let Some(ip) = hop else { break };
        client = ip.to_canonical();
        if !trusts(client) {
            break;
        }
    }
    Some(client)
}

/// The host the client addressed as the nearest proxy reports it, for a request from `peer`
/// carrying `headers`, `proxy` the server's `behind_proxy`. `None` without `proxy`, from a peer
/// outside its ranges, or when the nearest proxy reports no host: the request's own `Host`, which
/// that proxy sent, is then the one to read.
///
/// The nearest proxy is the trusted peer, and what it reports is the rightmost value: under
/// `XForwardedFor`, the last comma-separated element of `X-Forwarded-Host`'s last field line;
/// under `Forwarded`, the `host` parameter of the header's rightmost element, a `host` in any
/// element to its left never read, since the client or a proxy further out wrote it. A rightmost
/// element whose quote is left open, holding two `host` parameters, or a `host` whose quoted
/// string does not close, is answered `""`, a host that compares equal to no origin, rather than
/// falling back to `Host`.
pub(crate) fn forwarded_host(peer: Option<SocketAddr>, headers: &HeaderMap, proxy: Option<&BehindProxy>) -> Option<String> {
    let BehindProxy { header, trusted } = proxy?;
    let peer = peer?.ip().to_canonical();
    if !trusted.iter().any(|range| range.contains(peer)) {
        return None;
    }
    match header {
        ProxyHeader::XForwardedFor => {
            let line = headers.get_all(X_FORWARDED_HOST).iter().next_back()?;
            let text = String::from_utf8_lossy(line.as_bytes());
            text.split(',').map(trim_ows).rfind(|element| !element.is_empty()).map(str::to_owned)
        }
        ProxyHeader::Forwarded => {
            let line = headers.get_all(FORWARDED).iter().next_back()?;
            let text = String::from_utf8_lossy(line.as_bytes());
            let element = forwarded_elements(&text).into_iter().map(trim_ows).rfind(|element| !element.is_empty())?;
            forwarded_host_param(element)
        }
    }
}

/// The `host` parameter of one `Forwarded` element: `None` without one, `""` for one that cannot
/// be read as a single value.
fn forwarded_host_param(element: &str) -> Option<String> {
    let (pairs, open) = split_quoted(element, ';');
    if open {
        return Some(String::new());
    }
    let mut host = None;
    for pair in pairs.into_iter().map(trim_ows).filter(|pair| !pair.is_empty()) {
        let Some((name, value)) = pair.split_once('=') else { continue };
        if name.trim_matches([' ', '\t']).eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Some(String::new());
            }
            host = Some(unquoted(value.trim_matches([' ', '\t'])).unwrap_or_default());
        }
    }
    host
}

/// One `X-Forwarded-For` field line's hops, left to right: comma-separated addresses, each an
/// IPv4 or IPv6 address, bracketed or not, with or without a port. Empty elements are skipped, as
/// RFC 9110 §5.6.1 has a recipient do. An element carrying a byte outside visible ASCII names
/// nothing, and the rest of its line is still read: a proxy appends to the line a client sent,
/// so a byte the client wrote must not hide the address the proxy wrote after it.
fn xff_line(line: &HeaderValue) -> Vec<Option<IpAddr>> {
    let text = String::from_utf8_lossy(line.as_bytes());
    text.split(',').map(trim_ows).filter(|element| !element.is_empty()).map(xff_hop).collect()
}

fn xff_hop(element: &str) -> Option<IpAddr> {
    if let Ok(ip) = element.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Ok(addr) = element.parse::<SocketAddr>() {
        return Some(addr.ip());
    }
    element.strip_prefix('[')?.strip_suffix(']')?.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

/// One `Forwarded` field line's hops, left to right, one per element: its `for` node's address,
/// or `None` for an element without exactly one `for`, a node naming no address, or text off the
/// grammar of RFC 7239 §4. A quoted string's commas and semicolons do not split it. A line whose
/// quoted string is left open at its end is split at every comma, quotes or not, and the element
/// holding the open quote names nothing: a proxy writes its own element whole and appends it at
/// the right of the line a client sent, so only the client's part can leave a quote open, and
/// reading the quote as open would make the proxy's element part of the client's value. An
/// element carrying a byte outside visible ASCII names nothing, as in `X-Forwarded-For`, and the
/// rest of its line is still read.
fn forwarded_line(line: &HeaderValue) -> Vec<Option<IpAddr>> {
    let text = String::from_utf8_lossy(line.as_bytes());
    forwarded_elements(&text).into_iter().map(trim_ows).filter(|element| !element.is_empty()).map(forwarded_element).collect()
}

/// One `Forwarded` field line's elements, as [`forwarded_line`] splits them.
fn forwarded_elements(text: &str) -> Vec<&str> {
    let (elements, open) = split_quoted(text, ',');
    if open { text.split(',').collect() } else { elements }
}

fn forwarded_element(element: &str) -> Option<IpAddr> {
    let (pairs, open) = split_quoted(element, ';');
    if open {
        return None;
    }
    let mut node = None;
    for pair in pairs.into_iter().map(trim_ows) {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=')?;
        if name.trim_matches([' ', '\t']).eq_ignore_ascii_case("for") {
            if node.is_some() {
                return None;
            }
            node = Some(unquoted(value.trim_matches([' ', '\t']))?);
        }
    }
    forwarded_node(&node?)
}

/// RFC 7239 §6: `nodename [ ":" node-port ]`, the name an IPv4 address, an IPv6 address in
/// brackets, `unknown` or an obfuscated `_name`; only an address answers.
fn forwarded_node(node: &str) -> Option<IpAddr> {
    let (ip, port) = match node.strip_prefix('[') {
        Some(rest) => {
            let (v6, port) = rest.split_once(']')?;
            (IpAddr::V6(v6.parse().ok()?), port)
        }
        None => match node.split_once(':') {
            Some((v4, _)) => (IpAddr::V4(v4.parse().ok()?), &node[v4.len()..]),
            None => (IpAddr::V4(node.parse().ok()?), ""),
        },
    };
    match port.strip_prefix(':') {
        None if port.is_empty() => Some(ip),
        Some(port) if node_port(port) => Some(ip),
        _ => None,
    }
}

/// `node-port = port / obfport`: one to five digits, or `_` and one or more of `ALPHA`, `DIGIT`,
/// `.`, `_` and `-`.
fn node_port(port: &str) -> bool {
    let digits = (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit());
    let obfuscated = port
        .strip_prefix('_')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')));
    digits || obfuscated
}

/// A `token` as it stands, or a `quoted-string`'s content with each quoted pair unescaped; `None`
/// for a quoted string not closed at the value's end.
fn unquoted(value: &str) -> Option<String> {
    let Some(rest) = value.strip_prefix('"') else { return Some(value.to_owned()) };
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return chars.as_str().is_empty().then_some(out),
            c => out.push(c),
        }
    }
    None
}

/// `text` split at each `separator` outside a quoted string, and whether a quoted string is still
/// open at its end.
fn split_quoted(text: &str, separator: char) -> (Vec<&str>, bool) {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == separator && !quoted {
            parts.push(&text[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&text[start..]);
    (parts, quoted)
}

fn trim_ows(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use http::HeaderMap;
    use http::header::HeaderValue;

    use super::{BehindProxy, Cidr, ProxyHeader, client_ip};

    const PROXY: &str = "10.0.0.1";

    fn trusted() -> Vec<Cidr> {
        ["10.0.0.0/8", "2001:db8:ffff::/48"].into_iter().map(|range| range.parse().expect("a range")).collect()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    fn peer(text: &str) -> Option<SocketAddr> {
        Some(SocketAddr::new(ip(text), 443))
    }

    /// The headers named, one field line per entry, in order.
    fn headers(lines: &[(&str, &[u8])]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in lines {
            map.append(http::HeaderName::from_bytes(name.as_bytes()).expect("a name"), HeaderValue::from_bytes(value).expect("a value"));
        }
        map
    }

    /// What a row pins, the peer, the header lines, the answer.
    type Row = (&'static str, &'static str, &'static [(&'static str, &'static [u8])], &'static str);

    fn behind(header: ProxyHeader) -> BehindProxy {
        BehindProxy { header, trusted: trusted() }
    }

    /// Each row's answer behind proxies writing `header`.
    fn walks(header: ProxyHeader, rows: &[Row]) {
        for (what, from, lines, expected) in rows {
            let answer = client_ip(peer(from), &headers(lines), Some(&behind(header)));
            assert_eq!(answer, Some(ip(expected)), "{what}: {header:?}, peer {from}, headers {lines:?}");
        }
    }

    #[test]
    fn the_x_forwarded_for_walk_answers_the_first_untrusted_hop_from_the_right() {
        walks(ProxyHeader::XForwardedFor, &[
            ("an untrusted peer is answered", "203.0.113.9", &[], "203.0.113.9"),
            ("a spoofed header from an untrusted peer is not read", "203.0.113.9", &[("x-forwarded-for", b"198.51.100.1")], "203.0.113.9"),
            ("a trusted peer with no header is answered", PROXY, &[], PROXY),
            ("one hop behind a trusted proxy", PROXY, &[("x-forwarded-for", b"198.51.100.7")], "198.51.100.7"),
            (
                "a spoofed leftmost entry behind a trusted proxy is passed over",
                PROXY,
                &[("x-forwarded-for", b"6.6.6.6, 198.51.100.7")],
                "198.51.100.7",
            ),
            ("trusted hops are walked past", PROXY, &[("x-forwarded-for", b"198.51.100.7, 10.1.1.1, 10.2.2.2")], "198.51.100.7"),
            ("every hop trusted answers the leftmost", PROXY, &[("x-forwarded-for", b"10.9.9.9, 10.1.1.1")], "10.9.9.9"),
            ("an empty X-Forwarded-For answers the peer", PROXY, &[("x-forwarded-for", b"")], PROXY),
            ("empty list elements are skipped", PROXY, &[("x-forwarded-for", b" , 198.51.100.7 ,, ")], "198.51.100.7"),
            ("whitespace around elements", PROXY, &[("x-forwarded-for", b"\t198.51.100.7 \t,  10.1.1.1  ")], "198.51.100.7"),
            ("an IPv4 hop with a port", PROXY, &[("x-forwarded-for", b"198.51.100.7:4711")], "198.51.100.7"),
            ("an IPv6 hop", PROXY, &[("x-forwarded-for", b"2001:db8::1")], "2001:db8::1"),
            ("a bracketed IPv6 hop with a port", PROXY, &[("x-forwarded-for", b"[2001:db8::1]:4711")], "2001:db8::1"),
            ("a bracketed IPv6 hop without a port", PROXY, &[("x-forwarded-for", b"[2001:db8::1]")], "2001:db8::1"),
            (
                "an unparseable hop ends the walk at the last hop passed",
                PROXY,
                &[("x-forwarded-for", b"198.51.100.7, garbage, 10.1.1.1")],
                "10.1.1.1",
            ),
            ("an unparseable rightmost hop answers the peer", PROXY, &[("x-forwarded-for", b"198.51.100.7, unknown")], PROXY),
            (
                "field lines are one list in order",
                PROXY,
                &[("x-forwarded-for", b"6.6.6.6"), ("x-forwarded-for", b"198.51.100.7, 10.1.1.1")],
                "198.51.100.7",
            ),
            (
                "an element that is not visible ASCII is a hop naming nothing",
                PROXY,
                &[("x-forwarded-for", b"198.51.100.7, \xff"), ("x-forwarded-for", b"10.1.1.1")],
                "10.1.1.1",
            ),
            (
                "a client's byte outside visible ASCII does not hide the address a proxy appended to its line",
                PROXY,
                &[("x-forwarded-for", b"\xff, 203.0.113.5")],
                "203.0.113.5",
            ),
            ("an IPv4-mapped peer is matched and answered as IPv4", "::ffff:10.0.0.1", &[("x-forwarded-for", b"198.51.100.7")], "198.51.100.7"),
            ("an untrusted IPv4-mapped peer is answered as IPv4", "::ffff:203.0.113.9", &[], "203.0.113.9"),
            ("an IPv4-mapped hop is matched and answered as IPv4", PROXY, &[("x-forwarded-for", b"198.51.100.7, ::ffff:10.1.1.1")], "198.51.100.7"),
        ]);
    }

    #[test]
    fn the_forwarded_walk_answers_the_first_untrusted_hop_from_the_right() {
        walks(ProxyHeader::Forwarded, &[
            ("an untrusted peer is answered", "203.0.113.9", &[], "203.0.113.9"),
            ("a spoofed Forwarded from an untrusted peer is not read", "203.0.113.9", &[("forwarded", b"for=198.51.100.1")], "203.0.113.9"),
            ("a trusted peer with no header is answered", PROXY, &[], PROXY),
            ("the Forwarded example of RFC 7239 4", PROXY, &[("forwarded", b"for=192.0.2.43, for=198.51.100.17;by=203.0.113.60;proto=http;host=example.com")], "198.51.100.17"),
            ("a quoted IPv6 node with a port", PROXY, &[("forwarded", b"for=\"[2001:db8:cafe::17]:4711\"")], "2001:db8:cafe::17"),
            ("a quoted IPv4 node with a port", PROXY, &[("forwarded", b"for=\"192.0.2.43:47011\"")], "192.0.2.43"),
            ("the parameter name is case-insensitive", PROXY, &[("forwarded", b"For=192.0.2.43")], "192.0.2.43"),
            ("pairs in any order", PROXY, &[("forwarded", b"proto=https;for=192.0.2.43;by=10.0.0.1")], "192.0.2.43"),
            ("an obfuscated port is accepted", PROXY, &[("forwarded", b"for=\"192.0.2.43:_port1\"")], "192.0.2.43"),
            ("unknown ends the walk", PROXY, &[("forwarded", b"for=192.0.2.43, for=unknown")], PROXY),
            ("an obfuscated node ends the walk", PROXY, &[("forwarded", b"for=192.0.2.43, for=_hidden, for=10.1.1.1")], "10.1.1.1"),
            ("an element without for ends the walk", PROXY, &[("forwarded", b"for=192.0.2.43, proto=https")], PROXY),
            ("an element with two for parameters names nothing", PROXY, &[("forwarded", b"for=192.0.2.43;for=10.1.1.1")], PROXY),
            ("an unbracketed IPv6 node names nothing", PROXY, &[("forwarded", b"for=\"2001:db8::1\"")], PROXY),
            ("a quoted comma does not split an element", PROXY, &[("forwarded", b"for=192.0.2.43;host=\"a,b\", for=10.1.1.1")], "192.0.2.43"),
            ("a quoted string left open names nothing", PROXY, &[("forwarded", b"for=\"198.51.100.1")], PROXY),
            (
                "a line with an unclosed quote is split at every comma",
                PROXY,
                &[("forwarded", b"for=\"198.51.100.1, for=192.0.2.43")],
                "192.0.2.43",
            ),
            (
                "a quote a client leaves open neither swallows what a proxy appends nor answers the client's for",
                PROXY,
                &[("forwarded", b"for=6.6.6.6;host=\"x, for=203.0.113.5")],
                "203.0.113.5",
            ),
            (
                "a quote opened inside a token neither swallows what a proxy appends nor answers the client's for",
                PROXY,
                &[("forwarded", b"for=6.6.6.6;x=a\"b, for=\"[2001:db8::5]\"")],
                "2001:db8::5",
            ),
            (
                "an escaped quote left open is split at every comma too",
                PROXY,
                &[("forwarded", b"for=\"6.6.6.6\\\", for=203.0.113.5")],
                "203.0.113.5",
            ),
            (
                "behind two appending proxies the walk passes the trusted one to the client",
                PROXY,
                &[("forwarded", b"for=\"6.6.6.6, for=203.0.113.5, for=10.1.1.1")],
                "203.0.113.5",
            ),
            ("Forwarded field lines are one list in order", PROXY, &[("forwarded", b"for=6.6.6.6"), ("forwarded", b"for=192.0.2.43")], "192.0.2.43"),
            ("an empty Forwarded answers the peer", PROXY, &[("forwarded", b"")], PROXY),
            (
                "a client's byte outside visible ASCII does not hide the element a proxy appended to its line",
                PROXY,
                &[("forwarded", b"for=\"\xff\", for=203.0.113.5")],
                "203.0.113.5",
            ),
            ("an IPv4-mapped peer is matched and answered as IPv4", "::ffff:10.0.0.1", &[("forwarded", b"for=198.51.100.7")], "198.51.100.7"),
            ("an IPv4-mapped hop is matched and answered as IPv4", PROXY, &[("forwarded", b"for=198.51.100.7, for=\"[::ffff:10.1.1.1]\"")], "198.51.100.7"),
            ("an IPv6 trusted range", "2001:db8:ffff::5", &[("forwarded", b"for=\"[2001:db8::9]\"")], "2001:db8::9"),
        ]);
    }

    /// The header not named is never read, whether the named one is absent, names nothing, or
    /// names an address, and whichever of the two the request carries first.
    #[test]
    fn only_the_named_header_is_read() {
        walks(ProxyHeader::XForwardedFor, &[
            ("a client's Forwarded behind a proxy appending X-Forwarded-For", PROXY, &[("forwarded", b"for=6.6.6.6"), ("x-forwarded-for", b"198.51.100.7")], "198.51.100.7"),
            ("an empty Forwarded does not hide X-Forwarded-For", PROXY, &[("forwarded", b""), ("x-forwarded-for", b"198.51.100.7")], "198.51.100.7"),
            ("a Forwarded alone answers the peer", PROXY, &[("forwarded", b"for=192.0.2.43")], PROXY),
            ("a Forwarded beside trusted hops answers the last of them", PROXY, &[("x-forwarded-for", b"10.1.1.1"), ("forwarded", b"for=192.0.2.43")], "10.1.1.1"),
        ]);
        walks(ProxyHeader::Forwarded, &[
            ("a client's X-Forwarded-For behind a proxy writing Forwarded", PROXY, &[("x-forwarded-for", b"6.6.6.6"), ("forwarded", b"for=192.0.2.43")], "192.0.2.43"),
            ("a Forwarded naming no address is not followed by X-Forwarded-For", PROXY, &[("forwarded", b"for=unknown"), ("x-forwarded-for", b"198.51.100.7")], PROXY),
            ("an X-Forwarded-For alone answers the peer", PROXY, &[("x-forwarded-for", b"198.51.100.7")], PROXY),
            ("an X-Forwarded-For beside trusted hops answers the last of them", PROXY, &[("forwarded", b"for=10.1.1.1"), ("x-forwarded-for", b"198.51.100.7")], "10.1.1.1"),
        ]);
    }

    #[test]
    fn unset_the_peer_is_answered_whatever_the_headers_say() {
        let lines = headers(&[("x-forwarded-for", b"198.51.100.7"), ("forwarded", b"for=192.0.2.43")]);
        assert_eq!(client_ip(peer(PROXY), &lines, None), Some(ip(PROXY)), "unset");
        for header in [ProxyHeader::Forwarded, ProxyHeader::XForwardedFor] {
            let nothing_trusted = BehindProxy { header, trusted: Vec::new() };
            assert_eq!(client_ip(peer(PROXY), &lines, Some(&nothing_trusted)), Some(ip(PROXY)), "{header:?} with no range");
        }
    }

    #[test]
    fn without_a_peer_address_there_is_no_answer() {
        let lines = headers(&[("x-forwarded-for", b"198.51.100.7")]);
        assert_eq!(client_ip(None, &lines, Some(&behind(ProxyHeader::XForwardedFor))), None);
        assert_eq!(client_ip(None, &lines, None), None);
    }

    /// What a row pins for the forwarded host: the header named, the peer, the header lines, the
    /// answer.
    type HostRow = (&'static str, ProxyHeader, &'static str, &'static [(&'static str, &'static [u8])], Option<&'static str>);

    #[test]
    fn the_forwarded_host_is_the_nearest_proxy_s_report_from_a_trusted_peer_alone() {
        use ProxyHeader::{Forwarded, XForwardedFor};
        let rows: &[HostRow] = &[
            ("one X-Forwarded-Host", XForwardedFor, PROXY, &[("x-forwarded-host", b"app.example")], Some("app.example")),
            ("the nearest proxy's value is the last", XForwardedFor, PROXY, &[("x-forwarded-host", b"evil.example, app.example")], Some("app.example")),
            ("the last field line", XForwardedFor, PROXY, &[("x-forwarded-host", b"evil.example"), ("x-forwarded-host", b"app.example:8443")], Some("app.example:8443")),
            ("empty elements skipped", XForwardedFor, PROXY, &[("x-forwarded-host", b"app.example, ")], Some("app.example")),
            ("none reported", XForwardedFor, PROXY, &[], None),
            ("an untrusted peer is not believed", XForwardedFor, "203.0.113.9", &[("x-forwarded-host", b"evil.example")], None),
            ("the other header's host is not read", XForwardedFor, PROXY, &[("forwarded", b"host=evil.example")], None),
            ("the rightmost element's host", Forwarded, PROXY, &[("forwarded", b"for=6.6.6.6;host=evil.example, for=198.51.100.7;host=app.example")], Some("app.example")),
            ("a quoted host with a port", Forwarded, PROXY, &[("forwarded", b"for=198.51.100.7;host=\"app.example:8443\";proto=https")], Some("app.example:8443")),
            ("a host left of the rightmost element is not read", Forwarded, PROXY, &[("forwarded", b"for=6.6.6.6;host=evil.example, for=198.51.100.7")], None),
            ("the last field line's rightmost element", Forwarded, PROXY, &[("forwarded", b"host=app.example"), ("forwarded", b"for=6.6.6.6")], None),
            ("two hosts name none", Forwarded, PROXY, &[("forwarded", b"host=a.example;host=b.example")], Some("")),
            ("an open quote in the rightmost element names none", Forwarded, PROXY, &[("forwarded", b"for=1.2.3.4;host=\"app.example")], Some("")),
            ("a client's open quote does not take the proxy's element", Forwarded, PROXY, &[("forwarded", b"host=\"evil.example, for=198.51.100.7;host=app.example")], Some("app.example")),
            ("an untrusted peer's Forwarded is not believed", Forwarded, "203.0.113.9", &[("forwarded", b"host=evil.example")], None),
            ("X-Forwarded-Host is not read under Forwarded", Forwarded, PROXY, &[("x-forwarded-host", b"evil.example")], None),
            (
                "a client's byte outside visible ASCII does not hide the host a proxy appended to its X-Forwarded-Host",
                XForwardedFor,
                PROXY,
                &[("x-forwarded-host", b"\xff, app.example")],
                Some("app.example"),
            ),
            (
                "a client's byte outside visible ASCII does not hide the element a proxy appended to its Forwarded",
                Forwarded,
                PROXY,
                &[("forwarded", b"for=\"\xff\";host=evil.example, for=198.51.100.7;host=app.example")],
                Some("app.example"),
            ),
        ];
        for (what, header, from, lines, expected) in rows {
            let answer = super::forwarded_host(peer(from), &headers(lines), Some(&behind(*header)));
            assert_eq!(answer.as_deref(), *expected, "{what}: {header:?}, peer {from}, headers {lines:?}");
        }
        let lines = headers(&[("x-forwarded-host", b"evil.example"), ("forwarded", b"host=evil.example")]);
        assert_eq!(super::forwarded_host(peer(PROXY), &lines, None), None, "unset");
        assert_eq!(super::forwarded_host(None, &lines, Some(&behind(XForwardedFor))), None, "no peer");
    }

    #[test]
    fn an_oversize_header_is_walked_to_its_first_untrusted_hop() {
        let mut spoofed: Vec<String> = (0..20_000).map(|i| format!("6.6.{}.{}", i / 256 % 256, i % 256)).collect();
        spoofed.push("198.51.100.7".to_owned());
        spoofed.extend((0..50).map(|i| format!("10.0.1.{i}")));
        let line = spoofed.join(", ");
        assert!(line.len() > 200_000, "the header is {} bytes", line.len());
        let answer = client_ip(peer(PROXY), &headers(&[("x-forwarded-for", line.as_bytes())]), Some(&behind(ProxyHeader::XForwardedFor)));
        assert_eq!(answer, Some(ip("198.51.100.7")));
    }

    #[test]
    fn ranges_parse_from_text() {
        let rows: &[(&str, &str)] = &[
            ("10.0.0.0/8", "10.0.0.0/8"),
            ("10.1.2.3/8", "10.0.0.0/8"),
            ("192.0.2.7", "192.0.2.7/32"),
            ("0.0.0.0/0", "0.0.0.0/0"),
            ("2001:db8::/32", "2001:db8::/32"),
            ("2001:db8::1", "2001:db8::1/128"),
            ("::/0", "::/0"),
            ("::ffff:10.0.0.0/104", "10.0.0.0/8"),
            ("::ffff:0.0.0.0/96", "0.0.0.0/0"),
            ("::ffff:0.0.0.0/95", "::fffe:0:0/95"),
            ("::ffff:10.1.2.3", "10.1.2.3/32"),
        ];
        for (text, expected) in rows {
            let range: Cidr = text.parse().unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(range.to_string(), *expected, "{text}");
        }
    }

    #[test]
    fn a_refused_range_names_its_text_and_why() {
        let rows: &[(&str, &str)] = &[
            ("10.0.0.0/33", "`10.0.0.0/33` is not a CIDR range: the prefix length 33 is over 32, the address's length in bits"),
            ("2001:db8::/129", "`2001:db8::/129` is not a CIDR range: the prefix length 129 is over 128, the address's length in bits"),
            ("10.0.0.0/99999999999", "`10.0.0.0/99999999999` is not a CIDR range: the prefix length 99999999999 is over 32, the address's length in bits"),
            ("10.0.0.0/", "`10.0.0.0/` is not a CIDR range: the part after `/` is not a prefix length in decimal digits"),
            ("10.0.0.0/+8", "`10.0.0.0/+8` is not a CIDR range: the part after `/` is not a prefix length in decimal digits"),
            ("10.0.0/8", "`10.0.0/8` is not a CIDR range: the part before `/` is not an IPv4 or IPv6 address"),
            (" 10.0.0.0/8", "` 10.0.0.0/8` is not a CIDR range: the part before `/` is not an IPv4 or IPv6 address"),
            ("", "`` is not a CIDR range: the part before `/` is not an IPv4 or IPv6 address"),
            ("[::1]/128", "`[::1]/128` is not a CIDR range: the part before `/` is not an IPv4 or IPv6 address"),
        ];
        for (text, expected) in rows {
            let error = text.parse::<Cidr>().expect_err(text);
            assert_eq!(error.to_string(), *expected);
            assert_eq!(error.input(), *text);
        }
        let built = Cidr::new(ip("10.0.0.0"), 40).expect_err("a /40 IPv4 range");
        assert_eq!(built.to_string(), "`10.0.0.0/40` is not a CIDR range: the prefix length 40 is over 32, the address's length in bits");
    }

    #[test]
    fn containment_follows_the_prefix() {
        let rows: &[(&str, &str, bool)] = &[
            ("10.0.0.0/8", "10.255.255.255", true),
            ("10.0.0.0/8", "11.0.0.0", false),
            ("10.0.0.0/8", "9.255.255.255", false),
            ("192.0.2.7/32", "192.0.2.7", true),
            ("192.0.2.7/32", "192.0.2.8", false),
            ("0.0.0.0/0", "203.0.113.1", true),
            ("0.0.0.0/0", "2001:db8::1", false),
            ("2001:db8::/32", "2001:db8:ffff::1", true),
            ("2001:db8::/32", "2001:db9::1", false),
            ("::/0", "2001:db8::1", true),
            ("::/0", "10.0.0.1", false),
            ("::/0", "::ffff:10.0.0.1", false),
            ("10.0.0.0/8", "::ffff:10.0.0.1", true),
            ("::ffff:10.0.0.0/104", "10.0.0.1", true),
        ];
        for (range, addr, expected) in rows {
            let range: Cidr = range.parse().expect("a range");
            assert_eq!(range.contains(ip(addr)), *expected, "{range} contains {addr}");
        }
    }
}
