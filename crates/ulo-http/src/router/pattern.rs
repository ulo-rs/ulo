//! Route patterns (transports DESIGN §3.2): `{name}` segments plus an optional trailing
//! `{*rest}`. A trailing slash is insignificant: the pattern and the request path both drop it,
//! except for `/`.
//!
//! `ulo-http-macros` checks a literal against the same grammar at compile time, so a malformed
//! literal such as `/u/{id` is a span error; the two parsers are kept in step by hand.
//!
//! A request path's segments are percent-decoded before they are compared or captured, as path
//! decoding rather than form decoding: `+` stays `+`, and an encoded slash, `%2F` or `%2f`, stays
//! as the client wrote it, so a `/` in a captured value is always a separator the client sent.
//! A decoded byte sequence that is not UTF-8 is captured with U+FFFD in place of each invalid
//! sequence.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use percent_encoding::percent_decode_str;

use crate::cx::PathParams;

/// A parsed route pattern.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    /// Normalized: no trailing slash except for `/`.
    pub(crate) raw: Arc<str>,
    pub(crate) segments: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Static(String),
    Param(Arc<str>),
    /// `{*rest}`, last only: the remaining path, one or more segments.
    Rest(Arc<str>),
}

/// Why a pattern does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError {
    pub(crate) pattern: String,
    pub(crate) reason: &'static str,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a route pattern: {}", self.pattern, self.reason)
    }
}

impl std::error::Error for PatternError {}

impl Pattern {
    pub(crate) fn parse(text: &str) -> Result<Pattern, PatternError> {
        let error = |reason| PatternError { pattern: text.to_owned(), reason };
        let segments = parse_segments(text, false).map_err(error)?;
        Ok(Pattern { raw: Arc::from(normalize(text)), segments })
    }

    /// `prefix` and `route` joined with exactly one `/` between them, as `.at(prefix)` applies a
    /// controller's prefix.
    pub(crate) fn join(prefix: &str, route: &str) -> String {
        let prefix = prefix.trim_end_matches('/');
        let route = route.trim_start_matches('/');
        let mut joined = String::with_capacity(prefix.len() + route.len() + 2);
        if !prefix.starts_with('/') {
            joined.push('/');
        }
        joined.push_str(prefix);
        if !route.is_empty() {
            if !joined.ends_with('/') {
                joined.push('/');
            }
            joined.push_str(route);
        }
        joined
    }

    /// The same method on two patterns that are equal after normalization, or that differ only in
    /// a parameter's name at one position, is a duplicate route.
    pub(crate) fn conflicts(&self, other: &Pattern) -> bool {
        self.segments.len() == other.segments.len()
            && self.segments.iter().zip(&other.segments).all(|pair| match pair {
                (Segment::Static(a), Segment::Static(b)) => a == b,
                (Segment::Param(_), Segment::Param(_)) | (Segment::Rest(_), Segment::Rest(_)) => true,
                _ => false,
            })
    }

    /// The parameters of `path`, percent-decoded, when it matches.
    pub(crate) fn matches(&self, path: &str) -> Option<PathParams> {
        let path = normalize(path);
        self.match_split(path, &split(path))
    }

    /// `matches` over a path already normalized and split, so the router splits a request path
    /// once for every pattern it tries. A parameter or a rest never captures an empty segment.
    pub(crate) fn match_split(&self, path: &str, pieces: &[(usize, &str)]) -> Option<PathParams> {
        let mut pairs = Vec::new();
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Rest(name) => {
                    let &(offset, piece) = pieces.get(index)?;
                    if piece.is_empty() {
                        return None;
                    }
                    pairs.push((Arc::clone(name), decode(&path[offset..]).into_owned()));
                    return Some(PathParams { pairs });
                }
                Segment::Static(text) => {
                    let &(_, piece) = pieces.get(index)?;
                    if !static_matches(text, piece) {
                        return None;
                    }
                }
                Segment::Param(name) => {
                    let &(_, piece) = pieces.get(index)?;
                    if piece.is_empty() {
                        return None;
                    }
                    pairs.push((Arc::clone(name), decode(piece).into_owned()));
                }
            }
        }
        (pieces.len() == self.segments.len()).then_some(PathParams { pairs })
    }

    fn shape(&self) -> Shape<'_> {
        match self.segments.split_last() {
            Some((Segment::Rest(_), fixed)) => Shape { fixed, open: true },
            _ => Shape { fixed: &self.segments, open: false },
        }
    }

    pub(crate) fn param_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Param(name) | Segment::Rest(name) => Some(&**name),
            Segment::Static(_) => None,
        })
    }

    /// The precedence key among patterns that match one path: compared left to right, a static
    /// segment ranks before a parameter and a parameter before a rest, so `/users/me` wins over
    /// `/users/{id}` whatever order they were declared in.
    pub(crate) fn precedence(&self) -> Vec<u8> {
        self.segments
            .iter()
            .map(|segment| match segment {
                Segment::Static(_) => 0,
                Segment::Param(_) => 1,
                Segment::Rest(_) => 2,
            })
            .collect()
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// A pre-dispatch scope, `/admin/*`, matched against the request path. `*` as the last segment
/// matches one or more segments; a scope without one matches its pattern exactly. When the server
/// prepares it is also compared with each route pattern, so a route whose every request it matches
/// needs no check per request.
#[derive(Clone, Debug)]
pub(crate) struct ScopePattern {
    pub(crate) raw: Arc<str>,
    /// The segments before a final `*` or `{*rest}`, which `wildcard` records.
    segments: Vec<Segment>,
    wildcard: bool,
}

impl ScopePattern {
    pub(crate) fn parse(text: &str) -> Result<ScopePattern, PatternError> {
        let error = |reason| PatternError { pattern: text.to_owned(), reason };
        let mut segments = parse_segments(text, true).map_err(error)?;
        let wildcard = matches!(segments.last(), Some(Segment::Rest(_)));
        if wildcard {
            segments.pop();
        }
        Ok(ScopePattern { raw: Arc::from(normalize(text)), segments, wildcard })
    }

    fn shape(&self) -> Shape<'_> {
        Shape { fixed: &self.segments, open: self.wildcard }
    }

    /// Whether this scope matches every request path the route `pattern` matches.
    ///
    /// A scope's static segment covers only the same static segment, and its parameter covers a
    /// static segment or a parameter of the route, whatever the names. A route's `{*rest}` is
    /// covered only by the scope's wildcard, since it stands for one or more segments.
    pub(crate) fn covers(&self, pattern: &Pattern) -> bool {
        self.shape().contains(&pattern.shape())
    }

    /// Whether some request path matches both this scope and the route `pattern`: `/users/me`
    /// overlaps `/users/{id}` without covering it.
    pub(crate) fn overlaps(&self, pattern: &Pattern) -> bool {
        self.shape().overlaps(&pattern.shape())
    }

    /// Whether every request path `other` matches, this scope matches too.
    pub(crate) fn contains(&self, other: &ScopePattern) -> bool {
        self.shape().contains(&other.shape())
    }

    /// Whether this scope matches the request path `path`, compared as the router compares it.
    pub(crate) fn covers_path(&self, path: &str) -> bool {
        let path = normalize(path);
        let pieces = split(path);
        let fits = if self.wildcard { pieces.len() > self.segments.len() } else { pieces.len() == self.segments.len() };
        fits && self.segments.iter().zip(&pieces).all(|(segment, &(_, piece))| match segment {
            Segment::Static(text) => static_matches(text, piece),
            Segment::Param(_) => !piece.is_empty(),
            Segment::Rest(_) => false,
        })
    }
}

impl fmt::Display for ScopePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// The request paths a route pattern or a scope matches: exactly `fixed`, or with `open`, `fixed`
/// followed by one or more segments. `fixed` holds static segments and parameters only.
struct Shape<'s> {
    fixed: &'s [Segment],
    open: bool,
}

impl Shape<'_> {
    /// Whether every path `other` matches, this matches too.
    fn contains(&self, other: &Shape<'_>) -> bool {
        let (mine, theirs) = (self.fixed.len(), other.fixed.len());
        let fits = match (self.open, other.open) {
            (true, true) => theirs >= mine,
            (true, false) => theirs > mine,
            (false, false) => theirs == mine,
            (false, true) => false,
        };
        fits && self.fixed.iter().zip(other.fixed).all(|pair| match pair {
            (Segment::Static(a), Segment::Static(b)) => a == b,
            (Segment::Param(_), _) => true,
            _ => false,
        })
    }

    /// Whether some path matches both. Past the shorter `fixed`, the open one takes any segment.
    fn overlaps(&self, other: &Shape<'_>) -> bool {
        let (mine, theirs) = (self.fixed.len(), other.fixed.len());
        let fits = match (self.open, other.open) {
            (true, true) => true,
            (true, false) => theirs > mine,
            (false, true) => mine > theirs,
            (false, false) => theirs == mine,
        };
        fits && self.fixed.iter().zip(other.fixed).all(|pair| match pair {
            (Segment::Static(a), Segment::Static(b)) => a == b,
            _ => true,
        })
    }
}

/// Whether `path` holds a `.` or `..` segment (RFC 3986 §3.3), with each dot written raw, `%2E`
/// or `%2e`: `/a/..`, `/a/%2e%2E/b` and `/a/.%2e/b` do, `/a/.../b`, `/a/b./c` and `/a/%252e/b` do
/// not. Segments are those of the path as received, so `%2F` separates nothing here either.
pub(crate) fn has_dot_segment(path: &str) -> bool {
    path.split('/').any(is_dot_segment)
}

fn is_dot_segment(segment: &str) -> bool {
    let mut rest = segment.as_bytes();
    let mut dots = 0;
    while let [first, tail @ ..] = rest {
        rest = match (first, tail) {
            (b'.', _) => tail,
            (b'%', [b'2', e, after @ ..]) if e.eq_ignore_ascii_case(&b'e') => after,
            _ => return false,
        };
        dots += 1;
        if dots > 2 {
            return false;
        }
    }
    dots > 0
}

/// `path` without its trailing slash, except for `/`.
pub(crate) fn normalize(path: &str) -> &str {
    match path.strip_suffix('/') {
        Some(stripped) if !stripped.is_empty() => stripped,
        _ => path,
    }
}

/// The segments of a normalized path, each with its byte offset in it: none for `/`.
pub(crate) fn split(path: &str) -> Vec<(usize, &str)> {
    let Some(rest) = path.strip_prefix('/') else {
        return vec![(0, path)];
    };
    if rest.is_empty() {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let mut offset = 1;
    for piece in rest.split('/') {
        pieces.push((offset, piece));
        offset += piece.len() + 1;
    }
    pieces
}

/// The grammar: a leading `/`; segments of static text, `{name}`, or a final `{*name}`; no brace
/// elsewhere; no empty segment; no name twice. A name is non-empty, starts with `_` or a letter,
/// and continues with `_`, letters and digits. With `scope`, a final `*` is the wildcard, parsed
/// as a `Rest`.
///
/// `ulo-http-macros/src/pattern.rs` checks the same grammar with the same reasons, in the same
/// order; a change here is made there too.
fn parse_segments(text: &str, scope: bool) -> Result<Vec<Segment>, &'static str> {
    if !text.starts_with('/') {
        return Err("a pattern starts with `/`");
    }
    let normalized = normalize(text);
    if normalized == "/" {
        return Ok(Vec::new());
    }
    let pieces: Vec<&str> = normalized[1..].split('/').collect();
    let last = pieces.len() - 1;
    let mut segments = Vec::with_capacity(pieces.len());
    let mut names: Vec<&str> = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if piece.is_empty() {
            return Err("a segment between two `/` is empty");
        }
        let segment = if scope && piece == "*" {
            if index != last {
                return Err("`*` is the last segment only");
            }
            Segment::Rest(Arc::from("*"))
        } else if let Some(inner) = piece.strip_prefix('{') {
            let Some(inner) = inner.strip_suffix('}') else {
                return Err("a `{` opens a segment that does not end with `}`");
            };
            if let Some(name) = inner.strip_prefix('*') {
                check_name(name)?;
                if index != last {
                    return Err("`{*name}` is the last segment only");
                }
                named(&mut names, name)?;
                Segment::Rest(Arc::from(name))
            } else {
                check_name(inner)?;
                named(&mut names, inner)?;
                Segment::Param(Arc::from(inner))
            }
        } else if piece.contains(['{', '}']) {
            return Err("a brace appears outside a whole `{name}` segment");
        } else {
            // Decoded as a request segment is, so the router, a scope at runtime and the
            // comparison of a scope with a route pattern in `prepare` read one form whichever
            // side wrote `%C3%A9` and whichever wrote `é`.
            Segment::Static(decode(piece).into_owned())
        };
        segments.push(segment);
    }
    Ok(segments)
}

/// Records `name`, refusing one already recorded. Checked segment by segment, as
/// `ulo-http-macros` checks it, so both report the same first failure.
fn named<'t>(names: &mut Vec<&'t str>, name: &'t str) -> Result<(), &'static str> {
    if names.contains(&name) {
        return Err("a parameter name appears twice");
    }
    names.push(name);
    Ok(())
}

fn check_name(name: &str) -> Result<(), &'static str> {
    let mut chars = name.chars();
    match chars.next() {
        None => Err("a parameter name is empty"),
        Some(first) if first != '_' && !first.is_alphabetic() => Err("a parameter name starts with `_` or a letter"),
        Some(_) if chars.any(|c| c != '_' && !c.is_alphanumeric()) => Err("a parameter name holds only `_`, letters and digits"),
        Some(_) => Ok(()),
    }
}

/// A request segment against a static pattern segment, which `parse_segments` holds decoded:
/// equal as sent, or equal once decoded, so `/caf%C3%A9` matches the pattern `/café` and the
/// pattern `/caf%C3%A9` alike.
fn static_matches(text: &str, piece: &str) -> bool {
    text == piece || (piece.contains('%') && decode(piece) == text)
}

/// `piece` percent-decoded, an invalid UTF-8 sequence read as U+FFFD, with each encoded slash left
/// as written: a value captured from one segment then holds no `/`, and `root.join(value)` cannot
/// climb out of `root`. `%252F` decodes to the text `%2F` as well, so a captured `%2F` does not
/// say which of the two the client sent.
fn decode(piece: &str) -> Cow<'_, str> {
    let Some(first) = encoded_slash(piece) else {
        return percent_decode_str(piece).decode_utf8_lossy();
    };
    let mut decoded = String::with_capacity(piece.len());
    let mut rest = piece;
    let mut at = Some(first);
    while let Some(slash) = at {
        decoded.push_str(&percent_decode_str(&rest[..slash]).decode_utf8_lossy());
        decoded.push_str(&rest[slash..slash + 3]);
        rest = &rest[slash + 3..];
        at = encoded_slash(rest);
    }
    decoded.push_str(&percent_decode_str(rest).decode_utf8_lossy());
    Cow::Owned(decoded)
}

/// The byte offset of the first `%2F` or `%2f` in `text`.
fn encoded_slash(text: &str) -> Option<usize> {
    text.as_bytes().windows(3).position(|window| matches!(window, [b'%', b'2', b'F' | b'f']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoded_slash_stays_as_written_and_everything_else_decodes() {
        assert_eq!(decode("a%20b%2Fc%2fd%2520"), "a b%2Fc%2fd%20");
        assert_eq!(decode("..%2F..%2Fetc"), "..%2F..%2Fetc");
        assert_eq!(decode("%%2F"), "%%2F", "a `%` that starts no escape is kept, and the slash after it too");
        assert_eq!(decode("%C3%A9%2F%C3%A9"), "é%2Fé");
        assert_eq!(decode("plain"), "plain");
    }

    #[test]
    fn a_dot_segment_is_found_raw_and_encoded_and_nothing_else_is() {
        for path in ["/.", "/..", "/a/./b", "/a/../b", "/a/..", "/a/%2e", "/a/%2E%2e/b", "/a/.%2e", "/a/%2e./b", "/../"] {
            assert!(has_dot_segment(path), "{path}");
        }
        for path in ["/", "/a", "/a/.../b", "/a/b./c", "/a/.b", "/a/%252e/b", "/a/%2e%2e%2e", "/a/..%2Fb", "//", "/a/%2"] {
            assert!(!has_dot_segment(path), "{path}");
        }
    }
}
