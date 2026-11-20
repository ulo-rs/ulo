//! The handshake's `Origin` check (RFC 6455 §10.2, transports DESIGN §4.1): a browser sends
//! `Origin` on every WebSocket handshake and no CORS preflight before it, so the server decides
//! which pages may open a socket, refusing the rest with 403 before the upgrade.

use http::header::{HOST, ORIGIN};
use http::request::Parts;
use http::{HeaderMap, StatusCode};
use ulo_transport::ErrorKind;

use crate::connection::Refusal;

/// The pages a gateway's handshake admits, read from the `Origin` header a browser sends. A
/// handshake without `Origin` is admitted under every setting, since a client outside a browser
/// usually sends none and a browser always sends one. A refused handshake answers 403 before the
/// upgrade, its body naming `origins([..])`.
///
/// - [`SameOrigin`](Origins::SameOrigin), the default: the `Origin`'s host and port equal the
///   request's `Host`, as below.
/// - [`List`](Origins::List), written `.origins(["https://app.example"])`: the `Origin` is one
///   of the listed origins, each `http` or `https`, a host, and an optional port, compared with
///   the scheme, the host ignoring ASCII case and the port with the scheme's default filled in.
///   A same-origin page is admitted only when listed. `prepare` refuses an entry that is not such
///   an origin, `null` included; an empty list, which would admit only a handshake that sends no
///   `Origin`; and an entry whose host is not ASCII, naming its punycode form, since a browser
///   sends `Origin` in ASCII and such an entry could match no handshake: `https://bücher.example`
///   is written `https://xn--bcher-kva.example`. A trailing `.` is part of a listed host as of a
///   sent one, so `https://app.example.` admits only a page loaded from the dotted name.
/// - [`Any`](Origins::Any): every page.
///
/// Same-origin compares the `Origin`'s host and port with the request's authority, its `Host`, or
/// the authority of an absolute-form target, which RFC 9112 §3.2.2 has take its place. The scheme
/// is not compared: behind a TLS-terminating proxy the server cannot tell which one the browser
/// used. That admits one page another origin's rules would not, an `http://` page on the same
/// host and port as an `https://` one; a deployment that serves such a page it does not trust
/// lists its origins with `origins([..])`. Hosts compare ignoring ASCII case and are not
/// IDNA-mapped, so an internationalized name compares as the punycode a browser sends for it, and
/// a trailing `.` is part of the name. An explicit port must match, and a port missing on either
/// side is the default port of the `Origin`'s scheme, 80 for `http` and 443 for `https`:
///
/// | `Origin` | `Host` | Admitted |
/// |---|---|---|
/// | `http://a.example` or `https://a.example` | `a.example` | yes |
/// | `http://a.example` | `a.example:80` | yes |
/// | `https://a.example` | `a.example:443` | yes |
/// | `https://a.example` | `a.example:80` | no |
/// | `http://a.example` | `a.example:443` | no |
/// | `http://a.example:8080` | `a.example:8080` | yes |
/// | `http://a.example:8080` | `a.example` | no |
/// | `https://a.example:80` | `a.example` | no |
/// | `https://A.Example` | `a.example` | yes |
/// | `http://[::1]:3000` | `[::1]:3000` | yes |
/// | `http://b.example` | `a.example` | no |
/// | `null`, sent by a sandboxed frame or a `file:` page | any | no |
/// | not an `http` or `https` origin with a host | any | no |
///
/// On the HTTP server's port, a server set `behind_proxy(header, ranges)` compares the host its
/// proxy reports in place of `Host`, for a handshake whose peer is in `ranges`: with
/// `ProxyHeader::XForwardedFor` the last value of `X-Forwarded-Host`, with
/// `ProxyHeader::Forwarded` the `host=` of the header's rightmost element, each the nearest
/// proxy's, and `Host` when that proxy reports none. Without the setting, from any other peer,
/// and on a standalone server, a forwarded host is never read, so a client cannot choose the
/// host it is compared with. A deployment whose proxy rewrites `Host` and reports no forwarded
/// host lists its pages' origins with `origins([..])`.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Origins {
    #[default]
    SameOrigin,
    List(Vec<String>),
    Any,
}

impl<S: Into<String>, const N: usize> From<[S; N]> for Origins {
    fn from(origins: [S; N]) -> Self {
        Origins::List(origins.into_iter().map(Into::into).collect())
    }
}

impl<S: Into<String>> From<Vec<S>> for Origins {
    fn from(origins: Vec<S>) -> Self {
        Origins::List(origins.into_iter().map(Into::into).collect())
    }
}

/// [`Origins`] as a handshake checks it, its list parsed once in `prepare`.
#[derive(Clone, Debug)]
pub(crate) enum OriginCheck {
    SameOrigin,
    List(Vec<WebOrigin>),
    Any,
}

impl OriginCheck {
    /// The check `origins` declares, each list entry that is not an origin refused into
    /// `refusals`, which the caller reports naming its owner; a refused entry admits nothing.
    pub(crate) fn of(origins: &Origins, refusals: &mut Vec<String>) -> OriginCheck {
        match origins {
            Origins::SameOrigin => OriginCheck::SameOrigin,
            Origins::Any => OriginCheck::Any,
            Origins::List(list) if list.is_empty() => {
                refusals.push(
                    "`origins([])` lists no origin, so it admits only a handshake that sends no `Origin`, which a browser always \
                     sends; leave `origins` unset for `Origins::SameOrigin`, the default, which admits the server's own pages, \
                     or write `Origins::Any` to admit every page"
                        .to_owned(),
                );
                OriginCheck::List(Vec::new())
            }
            Origins::List(list) => OriginCheck::List(
                list.iter()
                    .filter_map(|entry| {
                        let Some(parsed) = WebOrigin::parse(entry) else {
                            refusals.push(format!(
                                "`origins([..])` lists `{entry}`, which is not an origin written `http://host` or \
                                 `https://host`, with an optional `:port` and nothing after it; `Origins::Any` admits every page"
                            ));
                            return None;
                        };
                        if !parsed.host.is_ascii() {
                            let ascii = match parsed.punycode() {
                                Some(ascii) => format!("write it `{ascii}`, as a browser sends it"),
                                None => "write its host in punycode, as a browser sends it".to_owned(),
                            };
                            refusals.push(format!(
                                "`origins([..])` lists `{entry}`, whose host is not ASCII; a browser sends `Origin` in ASCII, so the \
                                 entry matches no handshake: {ascii}"
                            ));
                            return None;
                        }
                        Some(parsed)
                    })
                    .collect(),
            ),
        }
    }

    /// `Ok` for a handshake this admits; the 403 otherwise. `forwarded` is the host the server's
    /// trusted proxy reports, `HandshakeAnswers::forwarded_host`, which same-origin compares in
    /// place of the request's own.
    pub(crate) fn check(&self, head: &Parts, forwarded: Option<&str>) -> Result<(), Refusal> {
        let mut sent = head.headers.get_all(ORIGIN).iter();
        let Some(origin) = sent.next() else { return Ok(()) };
        let origin = if sent.next().is_some() { None } else { origin.to_str().ok().and_then(WebOrigin::parse) };
        let admitted = match self {
            OriginCheck::Any => true,
            OriginCheck::List(list) => origin.is_some_and(|origin| list.contains(&origin)),
            OriginCheck::SameOrigin => origin.is_some_and(|origin| same_origin(&origin, head, forwarded)),
        };
        if admitted {
            return Ok(());
        }
        let why = match self {
            OriginCheck::List(_) => "is not one `origins([..])` lists",
            _ => "is not this server's own; `origins([..])` in the gateway defaults lists the origins of the pages it admits",
        };
        tracing::debug!(origin = ?head.headers.get(ORIGIN), host = ?effective_host(head, forwarded), path = head.uri.path(), "a WebSocket handshake's `Origin` was refused");
        Err(Refusal::new(StatusCode::FORBIDDEN, format!("{}: the handshake's `Origin` {why}", ErrorKind::Forbidden)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Http,
    Https,
}

/// An `http` or `https` origin, its host lowercased and its port the scheme's default when left
/// out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebOrigin {
    scheme: Scheme,
    host: String,
    port: u16,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

impl WebOrigin {
    /// `scheme://host[:port]` with nothing after it; `None` for anything else, `null` included.
    fn parse(text: &str) -> Option<WebOrigin> {
        let (scheme, rest) = text.split_once("://")?;
        let scheme = if scheme.eq_ignore_ascii_case("http") {
            Scheme::Http
        } else if scheme.eq_ignore_ascii_case("https") {
            Scheme::Https
        } else {
            return None;
        };
        if rest.contains(['/', '?', '#', '@']) {
            return None;
        }
        let (host, port) = host_port(rest)?;
        Some(WebOrigin { scheme, host: host.to_ascii_lowercase(), port: port.unwrap_or(scheme.default_port()) })
    }
}

impl WebOrigin {
    /// The origin with each label of its host that is not ASCII written in punycode, `xn--` and
    /// RFC 3492's encoding of the label lowercased, its port left out when it is the scheme's
    /// default. The labels are not otherwise IDNA-mapped, so a label a browser would normalise
    /// first, one in a decomposed form among them, may be written differently by the browser.
    /// `None` for a label too long to encode.
    fn punycode(&self) -> Option<String> {
        let labels: Option<Vec<String>> = self
            .host
            .split('.')
            .map(|label| if label.is_ascii() { Some(label.to_owned()) } else { encode_label(&label.to_lowercase()).map(|code| format!("xn--{code}")) })
            .collect();
        let scheme = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        let host = labels?.join(".");
        Some(if self.port == self.scheme.default_port() { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{}", self.port) })
    }
}

/// `label` in RFC 3492 punycode, without the `xn--` prefix; `None` when the encoding overflows,
/// which no label of a DNS name's length reaches.
fn encode_label(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    let input: Vec<u32> = label.chars().map(u32::from).collect();
    let mut output: String = label.chars().filter(char::is_ascii).collect();
    let basic = u32::try_from(output.len()).ok()?;
    if basic > 0 {
        output.push('-');
    }
    let (mut code, mut delta, mut bias, mut handled) = (0x80u32, 0u32, 72u32, basic);
    while (handled as usize) < input.len() {
        let next = *input.iter().filter(|&&point| point >= code).min()?;
        delta = delta.checked_add((next - code).checked_mul(handled + 1)?)?;
        code = next;
        for &point in &input {
            if point < code {
                delta = delta.checked_add(1)?;
            }
            if point == code {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { T_MIN } else if k >= bias + T_MAX { T_MAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    output.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        code = code.checked_add(1)?;
    }
    Some(output)
}

/// RFC 3492 §6.1's bias adaptation.
fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / 700 } else { delta / 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((36 - 1) * 26) / 2 {
        delta /= 36 - 1;
        k += 36;
    }
    k + (36 * delta) / (delta + 38)
}

/// The punycode digit for `value`, below 36: `a` to `z`, then `0` to `9`.
fn digit(value: u32) -> char {
    let value = u8::try_from(value).unwrap_or(0);
    char::from(if value < 26 { b'a' + value } else { b'0' + value - 26 })
}

/// `origin` against the request's effective host, the scheme not compared: a port missing on
/// either side is the default port of the `Origin`'s scheme.
fn same_origin(origin: &WebOrigin, head: &Parts, forwarded: Option<&str>) -> bool {
    let Some((host, port)) = effective_host(head, forwarded).and_then(host_port) else { return false };
    host.eq_ignore_ascii_case(&origin.host) && port.unwrap_or(origin.scheme.default_port()) == origin.port
}

/// The host and port the client addressed, which same-origin compares an `Origin` with:
/// `forwarded`, the host the server's trusted proxy reports, when there is one, else an
/// absolute-form target's authority, else the one `Host` header's. The caller reads `forwarded`
/// under `behind_proxy` alone, so without it this reads only what the request itself carries.
fn effective_host<'a>(head: &'a Parts, forwarded: Option<&'a str>) -> Option<&'a str> {
    if forwarded.is_some() {
        return forwarded;
    }
    if let Some(authority) = head.uri.authority() {
        return Some(authority.as_str());
    }
    single(&head.headers)
}

fn single(headers: &HeaderMap) -> Option<&str> {
    let mut hosts = headers.get_all(HOST).iter();
    let host = hosts.next()?;
    if hosts.next().is_some() {
        return None;
    }
    host.to_str().ok()
}

/// `host[:port]`, a bracketed IPv6 literal included; `None` for an empty host, an unbracketed
/// colon, or a port that is not one to five digits within `u16`.
fn host_port(text: &str) -> Option<(&str, Option<u16>)> {
    let (host, port) = if text.starts_with('[') {
        let end = text.find(']')?;
        let rest = &text[end + 1..];
        let port = if rest.is_empty() { None } else { Some(rest.strip_prefix(':')?) };
        (&text[..=end], port)
    } else {
        match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        }
    };
    if host.is_empty() || (!host.starts_with('[') && host.contains(':')) {
        return None;
    }
    let port = match port {
        None => None,
        Some(digits) if !digits.is_empty() && digits.len() <= 5 && digits.bytes().all(|byte| byte.is_ascii_digit()) => Some(digits.parse().ok()?),
        Some(_) => return None,
    };
    Some((host, port))
}

#[cfg(test)]
mod tests {
    use http::request::Parts;

    use super::{OriginCheck, Origins, WebOrigin};

    fn head(host: Option<&str>, origin: Option<&str>) -> Parts {
        let mut request = http::Request::builder().uri("/chat");
        if let Some(host) = host {
            request = request.header("host", host);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        request.body(()).expect("a request head").into_parts().0
    }

    fn admits(check: &OriginCheck, host: &str, origin: &str) -> bool {
        check.check(&head(Some(host), Some(origin)), None).is_ok()
    }

    /// The table on `Origins`, row by row.
    #[test]
    fn same_origin_compares_host_and_port_with_the_scheme_s_default_port_and_ignores_the_scheme() {
        let check = OriginCheck::SameOrigin;
        let rows = [
            ("http://a.example", "a.example", true),
            ("https://a.example", "a.example", true),
            ("http://a.example", "a.example:80", true),
            ("https://a.example", "a.example:443", true),
            ("https://a.example", "a.example:80", false),
            ("http://a.example", "a.example:443", false),
            ("http://a.example:8080", "a.example:8080", true),
            ("http://a.example:8080", "a.example", false),
            ("http://a.example:80", "a.example", true),
            ("https://a.example:443", "a.example", true),
            ("https://a.example:80", "a.example", false),
            ("http://a.example:443", "a.example", false),
            ("https://a.example:8443", "a.example:8443", true),
            ("https://a.example:8443", "a.example:8080", false),
            ("https://A.Example", "a.example", true),
            ("http://a.example", "A.EXAMPLE:80", true),
            ("http://[::1]:3000", "[::1]:3000", true),
            ("http://[::1]:3000", "[::2]:3000", false),
            ("http://b.example", "a.example", false),
            ("http://a.example.", "a.example", false),
            ("http://xn--caf-dma.example", "xn--caf-dma.example", true),
            ("null", "a.example", false),
            ("file://", "a.example", false),
            ("chrome-extension://abc", "abc", false),
            ("ws://a.example", "a.example", false),
            ("http://a.example/", "a.example", false),
            ("http://user@a.example", "a.example", false),
            ("http://a.example:+80", "a.example", false),
            ("http://a.example:", "a.example", false),
            ("http://a.example:99999", "a.example", false),
            ("http://", "a.example", false),
        ];
        for (origin, host, admitted) in rows {
            assert_eq!(admits(&check, host, origin), admitted, "Origin {origin} against Host {host}");
        }
    }

    #[test]
    fn a_handshake_without_origin_is_admitted_under_every_setting() {
        let mut refusals = Vec::new();
        for origins in [Origins::SameOrigin, Origins::from(["https://app.example"]), Origins::Any] {
            let check = OriginCheck::of(&origins, &mut refusals);
            assert!(check.check(&head(Some("a.example"), None), None).is_ok(), "{origins:?}");
            assert!(check.check(&head(None, None), None).is_ok(), "{origins:?} with no `Host` either");
        }
        assert!(refusals.is_empty());
    }

    #[test]
    fn same_origin_refuses_without_an_authority_or_with_two_hosts_or_two_origins() {
        let check = OriginCheck::SameOrigin;
        assert!(check.check(&head(None, Some("http://a.example")), None).is_err());
        let mut two_hosts = head(Some("a.example"), Some("http://a.example"));
        two_hosts.headers.append("host", http::HeaderValue::from_static("a.example"));
        assert!(check.check(&two_hosts, None).is_err());
        let mut two_origins = head(Some("a.example"), Some("http://a.example"));
        two_origins.headers.append("origin", http::HeaderValue::from_static("http://a.example"));
        assert!(check.check(&two_origins, None).is_err());
    }

    #[test]
    fn an_absolute_form_target_s_authority_takes_the_place_of_host() {
        let mut absolute = head(Some("other.example"), Some("http://a.example:8080"));
        absolute.uri = "http://a.example:8080/chat".parse().expect("a URI");
        assert!(OriginCheck::SameOrigin.check(&absolute, None).is_ok());
    }

    /// A host the trusted proxy reports takes the place of `Host` and of an absolute-form target;
    /// one naming no host compares equal to no origin.
    #[test]
    fn a_forwarded_host_takes_the_place_of_the_request_s_own() {
        let proxied = head(Some("backend:8080"), Some("https://app.example"));
        assert!(OriginCheck::SameOrigin.check(&proxied, None).is_err(), "the backend's `Host` is compared without one");
        assert!(OriginCheck::SameOrigin.check(&proxied, Some("app.example")).is_ok());
        assert!(OriginCheck::SameOrigin.check(&proxied, Some("app.example:443")).is_ok());
        assert!(OriginCheck::SameOrigin.check(&proxied, Some("other.example")).is_err());
        assert!(OriginCheck::SameOrigin.check(&proxied, Some("")).is_err(), "a report naming no host");
        let on_a_port = head(Some("backend:8080"), Some("https://app.example:8443"));
        assert!(OriginCheck::SameOrigin.check(&on_a_port, Some("app.example:8443")).is_ok(), "a report keeping the port");
        assert!(OriginCheck::SameOrigin.check(&on_a_port, Some("app.example")).is_err(), "a report dropping the port, as nginx's `$host` does");
        let mut absolute = head(Some("backend:8080"), Some("https://app.example"));
        absolute.uri = "http://backend:8080/chat".parse().expect("a URI");
        assert!(OriginCheck::SameOrigin.check(&absolute, Some("app.example")).is_ok(), "over an absolute-form target");
    }

    #[test]
    fn a_list_admits_its_origins_with_the_scheme_compared_and_the_default_port_filled_in() {
        let mut refusals = Vec::new();
        let check = OriginCheck::of(&Origins::from(["https://App.example", "http://localhost:5173"]), &mut refusals);
        assert!(refusals.is_empty(), "{refusals:?}");
        for (origin, admitted) in [
            ("https://app.example", true),
            ("https://app.example:443", true),
            ("http://app.example", false),
            ("https://app.example:8443", false),
            ("http://localhost:5173", true),
            ("http://localhost", false),
            ("null", false),
        ] {
            assert_eq!(admits(&check, "api.example", origin), admitted, "{origin}");
        }
        assert!(!admits(&check, "api.example", "https://api.example"), "a same-origin page is admitted only when listed");
    }

    #[test]
    fn any_admits_every_origin_null_included() {
        assert!(admits(&OriginCheck::Any, "a.example", "https://evil.example"));
        assert!(admits(&OriginCheck::Any, "a.example", "null"));
    }

    #[test]
    fn a_refusal_is_403_with_the_kind_ahead_of_a_reason_naming_the_setting() {
        let refused = OriginCheck::SameOrigin.check(&head(Some("a.example"), Some("https://evil.example")), None).expect_err("a cross-origin page");
        assert_eq!(refused.status(), http::StatusCode::FORBIDDEN);
        assert!(refused.reason().starts_with("forbidden: "), "{}", refused.reason());
        assert!(refused.reason().contains("`origins([..])`"), "{}", refused.reason());
    }

    #[test]
    fn an_entry_that_is_not_an_origin_is_refused_naming_it() {
        let mut refusals = Vec::new();
        OriginCheck::of(&Origins::from(["https://ok.example", "null", "app.example", "https://a.example/", "ftp://a.example"]), &mut refusals);
        assert_eq!(refusals.len(), 4, "{refusals:?}");
        for entry in ["`null`", "`app.example`", "`https://a.example/`", "`ftp://a.example`"] {
            assert!(refusals.iter().any(|refusal| refusal.contains(entry)), "{entry}: {refusals:?}");
        }
    }

    #[test]
    fn an_empty_list_is_refused_naming_the_default_and_any() {
        let mut refusals = Vec::new();
        OriginCheck::of(&Origins::List(Vec::new()), &mut refusals);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("`origins([])`") && refusals[0].contains("`Origins::SameOrigin`") && refusals[0].contains("`Origins::Any`"), "{refusals:?}");
    }

    /// An entry whose host is not ASCII is refused naming the punycode a browser sends, its port
    /// kept where it is not the scheme's default; an ASCII entry, punycode already, is accepted.
    #[test]
    fn an_entry_whose_host_is_not_ascii_is_refused_naming_its_punycode_form() {
        let mut refusals = Vec::new();
        let check = OriginCheck::of(
            &Origins::from(["https://bücher.example", "http://MÜNCHEN.example:8080", "https://xn--bcher-kva.example", "https://例え.テスト"]),
            &mut refusals,
        );
        assert_eq!(refusals.len(), 3, "{refusals:?}");
        assert!(refusals[0].contains("`https://bücher.example`") && refusals[0].contains("write it `https://xn--bcher-kva.example`"), "{refusals:?}");
        assert!(refusals[1].contains("write it `http://xn--mnchen-3ya.example:8080`"), "{refusals:?}");
        assert!(refusals[2].contains("write it `https://xn--r8jz45g.xn--zckzah`"), "{refusals:?}");
        assert!(admits(&check, "api.example", "https://xn--bcher-kva.example"));
    }

    /// RFC 3492 §7.1's sample strings, lowercased where the sample marks case, beside the labels
    /// the refusals name.
    #[test]
    fn a_label_encodes_as_rfc_3492_s_samples() {
        for (label, encoded) in [
            ("bücher", "bcher-kva"),
            ("münchen", "mnchen-3ya"),
            ("例え", "r8jz45g"),
            ("テスト", "zckzah"),
            ("他们为什么不说中文", "ihqwcrb4cv8a8dqg056pqjye"),
            ("ليهمابتكلموشعربي؟", "egbpdaj6bu4bxfgehfvwxn"),
            ("pročprostěnemluvíčesky", "proprostnemluvesky-uyb24dma41a"),
        ] {
            assert_eq!(super::encode_label(label).as_deref(), Some(encoded), "{label}");
        }
    }

    #[test]
    fn a_trailing_dot_is_part_of_a_listed_host() {
        let mut refusals = Vec::new();
        let check = OriginCheck::of(&Origins::from(["https://app.example."]), &mut refusals);
        assert!(refusals.is_empty(), "{refusals:?}");
        assert!(admits(&check, "api.example", "https://app.example."));
        assert!(!admits(&check, "api.example", "https://app.example"));
    }

    #[test]
    fn a_parsed_origin_lowercases_its_scheme_and_host_and_fills_in_the_default_port() {
        assert_eq!(WebOrigin::parse("HTTPS://App.Example"), WebOrigin::parse("https://app.example:443"));
        assert_ne!(WebOrigin::parse("http://app.example"), WebOrigin::parse("https://app.example"));
    }
}
