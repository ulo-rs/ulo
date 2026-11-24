//! Close codes (transports DESIGN §4.2): the codes a Close frame may carry, and the one each error
//! kind closes a refused connection with.

use std::error::Error;
use std::fmt;

use ulo_transport::{CallError, ErrorKind};

/// A close code a Close frame may carry, checked where it is built: 1000 to 1003, 1007 to 1014 and
/// 3000 to 4999.
///
/// Refused: 0 to 999, which RFC 6455 §7.4.2 leaves unused; 1004, which §7.4.1 reserves for a
/// meaning not yet defined, and 1005, 1006 and 1015, which it reserves for an endpoint's own
/// reports and no endpoint sends; 1016 to 2999, the protocol's range
/// with no code the IANA WebSocket Close Code Number Registry assigns; and 5000 and up, outside the
/// protocol. 3000 to 3999 is the registry's range for libraries and frameworks, and 4000 to 4999
/// is left to applications, a subprotocol's own codes among them, as graphql-transport-ws's 4401.
///
/// A close the gateway derives from an error kind, a refusal by kind or a hand-written gateway's
/// `on_message` panic, carries the kind in its reason, `not_found: no such room`, whatever its
/// code, 3000, 3003 and 3008 included: an application closes with those codes too, through
/// [`ConnectRefused::code`](crate::ConnectRefused::code), so the code alone does not tell the
/// framework's close from the application's. Every other close carries its reason alone: a code
/// set with [`ConnectRefused::code`](crate::ConnectRefused::code) or [`WithCloseCode`], the
/// drain's 1001, a slow consumer's 1008, [`Connection::close`](crate::Connection::close), and the
/// protocol's own 1002, 1003, 1007 and 1009. A client reads a kind from a reason only on a close
/// by kind.
///
/// ```
/// use ulo_ws::CloseCode;
///
/// const BANNED: CloseCode = match CloseCode::new(4003) {
///     Ok(code) => code,
///     Err(_) => panic!("4003 is an application's code"),
/// };
/// assert_eq!(BANNED.get(), 4003);
/// assert!(CloseCode::new(1005).is_err());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CloseCode(u16);

impl CloseCode {
    /// 1000: the purpose the connection was opened for is fulfilled.
    pub const NORMAL: CloseCode = CloseCode(1000);
    /// 1001: the server is going away, as at the drain.
    pub const GOING_AWAY: CloseCode = CloseCode(1001);
    /// 1003: a frame of a kind the endpoint does not accept, as a binary frame on a text-only
    /// gateway.
    pub const UNSUPPORTED_DATA: CloseCode = CloseCode(1003);
    /// 1008: a message violated the endpoint's policy, and no more specific code fits. The
    /// connect refusal of `BadRequest`, `NotFound`, `Conflict` and `Unprocessable`, its reason
    /// starting with the kind, and a slow consumer, whose reason `slow consumer` is no kind's.
    pub const POLICY_VIOLATION: CloseCode = CloseCode(1008);
    /// 1009: a message too large to process.
    pub const MESSAGE_TOO_BIG: CloseCode = CloseCode(1009);
    /// 1011: the server met a condition that kept it from fulfilling the request. The close of
    /// `Internal` and `Unimplemented`, its reason starting with the kind: a connect refusal, and a
    /// hand-written gateway's `on_message` panic as `internal: internal error`.
    pub const INTERNAL_ERROR: CloseCode = CloseCode(1011);
    /// 1013: try again later. The connect refusal of `TooManyRequests` and `Unavailable`, its
    /// reason starting with the kind, a connection over `max_connections` under `refuse = close`
    /// among them as `unavailable: too many connections`.
    pub const TRY_AGAIN_LATER: CloseCode = CloseCode(1013);
    /// 3000, the registry's `Unauthorized`: the connect refusal of `Unauthorized`.
    pub const UNAUTHORIZED: CloseCode = CloseCode(3000);
    /// 3003, the registry's `Forbidden`: the connect refusal of `Forbidden`, a connect guard's
    /// `Ok(false)` among them.
    pub const FORBIDDEN: CloseCode = CloseCode(3003);
    /// 3008, the registry's `Timeout`: the connect refusal of `Timeout`.
    pub const TIMEOUT: CloseCode = CloseCode(3008);

    /// The code `code`, refused as the type's documentation states.
    pub const fn new(code: u16) -> Result<CloseCode, CloseCodeError> {
        match code {
            1000..=1003 | 1007..=1014 | 3000..=4999 => Ok(CloseCode(code)),
            1004..=1006 | 1015 => Err(CloseCodeError::Reserved { code }),
            1016..=2999 => Err(CloseCodeError::Unassigned { code }),
            _ => Err(CloseCodeError::OutOfRange { code }),
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    /// The code a connect refusal of `kind` closes with: 3000, 3003 and 3008 for `Unauthorized`,
    /// `Forbidden` and `Timeout`, the registry's codes for them; 1008 for the client's other
    /// mistakes, `BadRequest`, `NotFound`, `Conflict` and `Unprocessable`, which the registry has
    /// no code for; 1013 for `TooManyRequests` and `Unavailable`; and 1011 for `Internal`,
    /// `Unimplemented` and any kind added later.
    pub const fn of_kind(kind: ErrorKind) -> CloseCode {
        match kind {
            ErrorKind::Unauthorized => CloseCode::UNAUTHORIZED,
            ErrorKind::Forbidden => CloseCode::FORBIDDEN,
            ErrorKind::Timeout => CloseCode::TIMEOUT,
            ErrorKind::BadRequest | ErrorKind::NotFound | ErrorKind::Conflict | ErrorKind::Unprocessable => {
                CloseCode::POLICY_VIOLATION
            }
            ErrorKind::TooManyRequests | ErrorKind::Unavailable => CloseCode::TRY_AGAIN_LATER,
            _ => CloseCode::INTERNAL_ERROR,
        }
    }

}

/// Every kind [`CloseCode::of_kind`] names. A kind added later closes with 1011, which
/// `Internal` and `Unimplemented` already share.
#[cfg(test)]
const KINDS: [ErrorKind; 11] = [
    ErrorKind::BadRequest,
    ErrorKind::Unauthorized,
    ErrorKind::Forbidden,
    ErrorKind::NotFound,
    ErrorKind::Conflict,
    ErrorKind::Unprocessable,
    ErrorKind::TooManyRequests,
    ErrorKind::Timeout,
    ErrorKind::Unavailable,
    ErrorKind::Unimplemented,
    ErrorKind::Internal,
];

impl fmt::Display for CloseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why [`CloseCode::new`], [`ConnectRefused::code`](crate::ConnectRefused::code) or
/// [`Connection::close`](crate::Connection::close) refused its arguments.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseCodeError {
    /// The reason is over 123 bytes of UTF-8.
    ReasonTooLong { len: usize },
    /// 1004, 1005, 1006 or 1015.
    Reserved { code: u16 },
    /// Outside 1000 to 4999.
    OutOfRange { code: u16 },
    /// 1016 to 2999.
    Unassigned { code: u16 },
}

const _: () = assert!(size_of::<CloseCodeError>() < 128, "clippy's `result_large_err` fires at 128 bytes");

impl fmt::Display for CloseCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloseCodeError::ReasonTooLong { len } => {
                write!(f, "a close reason is at most 123 bytes of UTF-8, and this one is {len}")
            }
            CloseCodeError::Reserved { code } => write!(f, "close code {code} is never sent in a Close frame"),
            CloseCodeError::OutOfRange { code } => write!(f, "close code {code} is outside 1000 to 4999"),
            CloseCodeError::Unassigned { code } => {
                write!(f, "close code {code} is in the protocol's range and the IANA registry assigns it no meaning")
            }
        }
    }
}

impl Error for CloseCodeError {}

/// Closes a connection the connect phase refuses with this error with a code of its own in place
/// of the one its kind maps to, as `with_grpc_code` names a gRPC code: a connect guard serving a
/// subprotocol with its own refusal codes closes with one of them. Read only where a connection is
/// refused, the reason being the error's message; a message handler's error answers the envelope,
/// whose `kind` the kind still decides, as it decides every other transport's rendering.
///
/// A refusal's own code has two spellings, one per place a refusal is written. Where the refusal
/// is an error, a connect guard's `Err` or a connect-phase interceptor's or error handler's, it is
/// a `CallError` naming the code with this trait: the error keeps its kind, which every error
/// handler after it reads, and under `refuse = handshake` is answered with the status HTTP sends
/// for it, the kind's or the one [`WithHttpStatus`](ulo_http::WithHttpStatus) names. Where the
/// refusal is the answer, the `Err` of [`OnConnect::on_connect`](crate::OnConnect::on_connect) or
/// [`Gateway::on_connect`](crate::Gateway::on_connect), or a connect-phase interceptor's or error
/// handler's [`ConnectReply::Refused`](crate::ConnectReply::Refused), it is a
/// [`ConnectRefused`](crate::ConnectRefused) built with
/// [`ConnectRefused::code`](crate::ConnectRefused::code), a code and a reason with no kind,
/// answered 403 under `refuse = handshake`; `ConnectRefused::from` turns a `CallError` carrying a
/// code set here into one with that code and the error's kind. A `ConnectRefused` boxed into an
/// `Err` is read as an error the connect phase does not recognise, and closes with 1011 as
/// `Internal`.
///
/// ```
/// use ulo_ws::{CallError, CloseCode, ErrorKind, WithCloseCode};
///
/// let code = CloseCode::new(4403).expect("an application's code");
/// let error = CallError::new(ErrorKind::Forbidden, "not a member").with_close_code(code);
/// assert_eq!(error.kind(), ErrorKind::Forbidden);
/// assert_eq!(error.close_code(), Some(code));
/// ```
///
/// Sealed: [`CallError`] is the one implementor, the store the gateway reads, so the trait can
/// gain a method without breaking a type outside this crate.
///
/// ```compile_fail,E0277
/// use ulo_ws::{CloseCode, WithCloseCode};
///
/// struct Mine;
///
/// impl WithCloseCode for Mine {
///     fn with_close_code(self, _code: CloseCode) -> Self {
///         self
///     }
///
///     fn close_code(&self) -> Option<CloseCode> {
///         None
///     }
/// }
/// ```
pub trait WithCloseCode: sealed::Sealed {
    fn with_close_code(self, code: CloseCode) -> Self;

    /// The code [`with_close_code`](Self::with_close_code) set, as it was set, so an error handler
    /// rebuilding the error keeps it. `None` when none was set, and a refusal closes with the
    /// kind's code ([`CloseCode::of_kind`]).
    fn close_code(&self) -> Option<CloseCode>;
}

mod sealed {
    pub trait Sealed {}

    impl Sealed for ulo_transport::CallError {}
}

impl WithCloseCode for CallError {
    fn with_close_code(self, code: CloseCode) -> Self {
        self.__with_close_code(code.get())
    }

    /// A stored number no [`CloseCode`] accepts, which only the doc-hidden store can write, reads
    /// as unset, as the refusal reads it.
    fn close_code(&self) -> Option<CloseCode> {
        self.__close_code().and_then(|code| CloseCode::new(code).ok())
    }
}

/// The longest close reason a Close frame carries (RFC 6455 §5.5.1: 125 bytes of payload, two of
/// them the code).
pub(crate) const REASON_LIMIT: usize = 123;

/// Refuses a reason over [`REASON_LIMIT`] bytes.
pub(crate) fn check_reason(reason: &str) -> Result<(), CloseCodeError> {
    if reason.len() > REASON_LIMIT {
        return Err(CloseCodeError::ReasonTooLong { len: reason.len() });
    }
    Ok(())
}

/// A close by `kind`, a refusal's or one mid-connection: the code [`CloseCode::of_kind`] gives,
/// and as the reason the kind, then `: ` and `message`. A `message` that already starts with the
/// kind and `: `, or is the kind alone, is the reason as it stands, and an empty one leaves the
/// kind alone. The message is cut to fit 123 bytes, the kind kept whole.
pub(crate) fn by_kind(kind: ErrorKind, message: &str) -> (CloseCode, String) {
    let code = CloseCode::of_kind(kind);
    let name = kind.to_string();
    let prefix = format!("{name}: ");
    let reason = if message.is_empty() {
        name
    } else if message == name || message.starts_with(&prefix) {
        truncated(message.to_owned())
    } else {
        let room = REASON_LIMIT - prefix.len();
        prefix + &truncated_to(message.to_owned(), room)
    };
    (code, reason)
}

/// `reason` cut to at most 123 bytes at a character boundary.
pub(crate) fn truncated(reason: String) -> String {
    truncated_to(reason, REASON_LIMIT)
}

/// `reason` cut to at most `limit` bytes at a character boundary.
pub(crate) fn truncated_to(mut reason: String, limit: usize) -> String {
    if reason.len() > limit {
        let mut end = limit;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `u16` against the accepted ranges, so a boundary moved by one fails.
    #[test]
    fn new_accepts_exactly_the_codes_a_frame_may_carry() {
        for code in 0..=u16::MAX {
            let accepted = matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999);
            assert_eq!(CloseCode::new(code).is_ok(), accepted, "close code {code}: {:?}", CloseCode::new(code));
        }
    }

    #[test]
    fn a_refused_code_names_its_range() {
        assert_eq!(CloseCode::new(1005), Err(CloseCodeError::Reserved { code: 1005 }));
        assert_eq!(CloseCode::new(1015), Err(CloseCodeError::Reserved { code: 1015 }));
        assert_eq!(CloseCode::new(1016), Err(CloseCodeError::Unassigned { code: 1016 }));
        assert_eq!(CloseCode::new(2999), Err(CloseCodeError::Unassigned { code: 2999 }));
        assert_eq!(CloseCode::new(999), Err(CloseCodeError::OutOfRange { code: 999 }));
        assert_eq!(CloseCode::new(5000), Err(CloseCodeError::OutOfRange { code: 5000 }));
    }

    /// A refusal by kind cuts its reason, the kind's prefix included, at the last character
    /// boundary within 123 bytes, never inside a character.
    #[test]
    fn a_reason_by_kind_is_cut_at_a_character_boundary() {
        let refused = crate::ConnectRefused::kind(ErrorKind::NotFound, "\u{20ac}".repeat(60));
        assert_eq!(refused.close_code(), CloseCode::POLICY_VIOLATION);
        assert!(refused.reason().starts_with("not_found: "), "{}", refused.reason());
        assert_eq!(refused.reason().len(), "not_found: ".len() + 3 * 37, "{}", refused.reason());
    }

    /// The prefix is kept whole and the message cut: `too_many_requests: ` is 19 bytes, leaving
    /// 104 for the message, 34 three-byte characters.
    #[test]
    fn a_long_reason_keeps_the_kind_whole() {
        let refused = crate::ConnectRefused::kind(ErrorKind::TooManyRequests, "\u{20ac}".repeat(60));
        assert_eq!(refused.reason(), format!("too_many_requests: {}", "\u{20ac}".repeat(34)));
        let ascii = crate::ConnectRefused::kind(ErrorKind::Internal, "x".repeat(200));
        assert_eq!(ascii.reason(), format!("internal: {}", "x".repeat(REASON_LIMIT - "internal: ".len())));
    }

    /// Every close by kind carries the kind, on a code it shares and on one it owns alike
    /// (Q34-41/49). Planted break: the prefix on shared codes alone, which leaves 3000, 3003 and
    /// 3008 reading as an application's close.
    #[test]
    fn every_close_by_kind_carries_the_kind() {
        let cases = [
            (ErrorKind::BadRequest, 1008, "bad_request: why"),
            (ErrorKind::NotFound, 1008, "not_found: why"),
            (ErrorKind::Conflict, 1008, "conflict: why"),
            (ErrorKind::Unprocessable, 1008, "unprocessable: why"),
            (ErrorKind::TooManyRequests, 1013, "too_many_requests: why"),
            (ErrorKind::Unavailable, 1013, "unavailable: why"),
            (ErrorKind::Internal, 1011, "internal: why"),
            (ErrorKind::Unimplemented, 1011, "unimplemented: why"),
            (ErrorKind::Unauthorized, 3000, "unauthorized: why"),
            (ErrorKind::Forbidden, 3003, "forbidden: why"),
            (ErrorKind::Timeout, 3008, "timeout: why"),
        ];
        assert_eq!(cases.len(), KINDS.len(), "a case per kind");
        for (kind, code, reason) in cases {
            let refused = crate::ConnectRefused::kind(kind, "why");
            assert_eq!((refused.close_code().get(), refused.reason()), (code, reason), "{kind:?}");
        }
    }

    /// A message already starting with its own kind's prefix carries it once, cut to 123 bytes
    /// with the prefix whole, and a message that is the kind alone, as a connect guard's `Ok(false)`
    /// writes it, or is empty, is the kind alone (Q30-5); another kind's prefix is the message's
    /// text and gains the kind's. Planted break: `forbidden` prefixed into `forbidden: forbidden`.
    #[test]
    fn a_message_already_carrying_the_prefix_is_not_prefixed_again() {
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::NotFound, "not_found: x").reason(), "not_found: x");
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::Forbidden, "forbidden").reason(), "forbidden");
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::Forbidden, "").reason(), "forbidden");
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::Forbidden, "forbidden: x").reason(), "forbidden: x");
        let long = format!("internal: {}", "x".repeat(200));
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::Internal, long.clone()).reason(), &long[..REASON_LIMIT]);
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::NotFound, "conflict: x").reason(), "not_found: conflict: x");
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::NotFound, "not_found:x").reason(), "not_found: not_found:x");
    }

    /// A code an error names through `WithCloseCode` carries the message alone, whatever the kind.
    #[test]
    fn an_error_s_own_code_carries_the_message_alone() {
        let code = CloseCode::new(4013).expect("an application's code");
        let refused = crate::ConnectRefused::from(CallError::new(ErrorKind::Unavailable, "why").with_close_code(code));
        assert_eq!((refused.close_code(), refused.reason()), (code, "why"));
    }

    /// The code reads back as it was set, and a number no `CloseCode` accepts reads back unset,
    /// as the refusal reads it.
    #[test]
    fn close_code_answers_the_code_as_set() {
        assert_eq!(CallError::new(ErrorKind::Forbidden, "why").close_code(), None);
        for number in [1000, 1003, 1014, 3000, 4403, 4999] {
            let code = CloseCode::new(number).expect("a code a Close frame carries");
            assert_eq!(CallError::new(ErrorKind::Forbidden, "why").with_close_code(code).close_code(), Some(code), "{number}");
        }
        for written in [0, 1005, 2000, 5000] {
            let call = CallError::new(ErrorKind::Forbidden, "why").__with_close_code(written);
            assert_eq!(call.close_code(), None, "{written}");
            assert_eq!(crate::ConnectRefused::from(call).close_code(), CloseCode::FORBIDDEN, "{written}");
        }
    }
}

#[cfg(test)]
mod checker_tests {
    use super::*;

    /// Every kind against every message shape: the reason starts with the kind and `: ` or is the
    /// kind alone, fits 123 bytes, never repeats the prefix it was given, and keeps as much of the
    /// message as fits after the prefix, cut at a character boundary.
    #[test]
    fn by_kind_over_every_kind_and_message_shape() {
        let long_ascii = "y".repeat(300);
        let long_multi = "\u{1F600}".repeat(40);
        for kind in KINDS {
            let name = kind.to_string();
            let prefix = format!("{name}: ");
            let shapes: Vec<String> = vec![
                String::new(),
                name.clone(),
                format!("{name}: x"),
                format!("{name}:x"),
                format!("why {name} mid-way"),
                "\u{e9}\u{4e2d}\u{1F600}".to_owned(),
                long_ascii.clone(),
                long_multi.clone(),
                format!("{prefix}{long_multi}"),
                name.to_uppercase(),
                " ".to_owned(),
            ];
            for message in shapes {
                let (code, reason) = by_kind(kind, &message);
                assert_eq!(code, CloseCode::of_kind(kind));
                assert!(reason.len() <= REASON_LIMIT, "{kind:?} {message:?}: {} bytes", reason.len());
                assert!(reason == name || reason.starts_with(&prefix), "{kind:?} {message:?}: {reason:?}");
                assert!(!reason.starts_with(&format!("{prefix}{prefix}")), "{kind:?} {message:?}: doubled {reason:?}");
                assert_ne!(reason, format!("{prefix}{name}"), "{kind:?} {message:?}: doubled {reason:?}");
                let expected = if message.is_empty() || message == name {
                    name.clone()
                } else if message.starts_with(&prefix) {
                    truncated(message.clone())
                } else {
                    let mut end = (REASON_LIMIT - prefix.len()).min(message.len());
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{prefix}{}", &message[..end])
                };
                assert_eq!(reason, expected, "{kind:?} {message:?}");
            }
        }
    }

    /// The codes the framework closes with for a kind, and an application's own code with the
    /// same number: the application's reason is its text alone, so the two are told apart by the
    /// prefix unless the application writes it.
    #[test]
    fn an_application_3003_is_told_from_the_framework_s() {
        let framework = crate::ConnectRefused::kind(ErrorKind::Forbidden, "not today");
        let app = crate::ConnectRefused::code(CloseCode::FORBIDDEN, "not today").expect("a reason within 123 bytes");
        assert_eq!(framework.close_code(), app.close_code());
        assert_eq!(framework.reason(), "forbidden: not today");
        assert_eq!(app.reason(), "not today");
        let guard_false = crate::ConnectRefused::kind(ErrorKind::Forbidden, "forbidden");
        let app_forbidden = crate::ConnectRefused::code(CloseCode::FORBIDDEN, "forbidden").expect("a reason");
        assert_eq!(guard_false.reason(), app_forbidden.reason(), "the kind alone reads the same from either");
        for (kind, code) in [(ErrorKind::BadRequest, 1008), (ErrorKind::Unavailable, 1013), (ErrorKind::Internal, 1011)] {
            assert_eq!(crate::ConnectRefused::kind(kind, "m").close_code().get(), code);
        }
    }
}
