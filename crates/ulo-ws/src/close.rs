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
    /// starting with the kind, and a slow consumer.
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

    /// More than one kind closes with this code under [`of_kind`](Self::of_kind), so a refusal by
    /// kind with it carries the kind in its reason.
    pub(crate) fn is_shared(self) -> bool {
        KINDS.iter().filter(|kind| CloseCode::of_kind(**kind) == self).count() > 1
    }
}

/// Every kind [`CloseCode::of_kind`] names. A kind added later closes with 1011, which
/// `Internal` and `Unimplemented` already share.
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
/// ```
/// use ulo_ws::{CallError, CloseCode, ErrorKind, WithCloseCode};
///
/// let code = CloseCode::new(4403).expect("an application's code");
/// let error = CallError::new(ErrorKind::Forbidden, "not a member").with_close_code(code);
/// assert_eq!(error.kind(), ErrorKind::Forbidden);
/// ```
pub trait WithCloseCode {
    fn with_close_code(self, code: CloseCode) -> Self;
}

impl WithCloseCode for CallError {
    fn with_close_code(self, code: CloseCode) -> Self {
        self.__with_close_code(code.get())
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
/// and as the reason `message`, after the kind and `: ` when more than one kind closes with that
/// code and `message` does not already start with them. The message is cut to fit 123 bytes, the
/// kind kept whole.
pub(crate) fn by_kind(kind: ErrorKind, message: &str) -> (CloseCode, String) {
    let code = CloseCode::of_kind(kind);
    let prefix = format!("{kind}: ");
    let reason = if code.is_shared() && !message.starts_with(&prefix) {
        let message = truncated_to(message.to_owned(), REASON_LIMIT - prefix.len());
        prefix + &message
    } else {
        truncated(message.to_owned())
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

    /// Every code more than one kind closes with carries the kind; a code one kind owns carries
    /// the message alone.
    #[test]
    fn a_shared_code_carries_the_kind_and_an_owned_one_does_not() {
        let cases = [
            (ErrorKind::BadRequest, 1008, "bad_request: why"),
            (ErrorKind::NotFound, 1008, "not_found: why"),
            (ErrorKind::Conflict, 1008, "conflict: why"),
            (ErrorKind::Unprocessable, 1008, "unprocessable: why"),
            (ErrorKind::TooManyRequests, 1013, "too_many_requests: why"),
            (ErrorKind::Unavailable, 1013, "unavailable: why"),
            (ErrorKind::Internal, 1011, "internal: why"),
            (ErrorKind::Unimplemented, 1011, "unimplemented: why"),
            (ErrorKind::Unauthorized, 3000, "why"),
            (ErrorKind::Forbidden, 3003, "why"),
            (ErrorKind::Timeout, 3008, "why"),
        ];
        for (kind, code, reason) in cases {
            let refused = crate::ConnectRefused::kind(kind, "why");
            assert_eq!((refused.close_code().get(), refused.reason()), (code, reason), "{kind:?}");
        }
    }

    /// A message already starting with its own kind's prefix carries it once, cut to 123 bytes
    /// with the prefix whole; another kind's prefix is the message's text and gains the kind's.
    #[test]
    fn a_message_already_carrying_the_prefix_is_not_prefixed_again() {
        assert_eq!(crate::ConnectRefused::kind(ErrorKind::NotFound, "not_found: x").reason(), "not_found: x");
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
}
