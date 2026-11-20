//! The call a frame names, read from the frame's leading bytes alone: a server answers a frame
//! over its `max_frame` under the frame's `id` without decoding the body past them, which it reads
//! and drops.
//!
//! The frame is a map whose entries come in any order, so the `t` and `id` entries are found only
//! where they precede everything the leading bytes cut off. `ulo-rpc`'s codec writes `t` and `id`
//! first; a caller writing ahead of `id` a `d` that runs past the leading bytes names no call in
//! them.

use ulo_rpc::Codec;

/// The frames that name a call a server answers: a request, a streamed request's `open`, and a
/// streamed request's item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Calling {
    Req,
    Open,
    In,
}

impl Calling {
    fn of(t: &[u8]) -> Option<Calling> {
        match t {
            b"req" => Some(Calling::Req),
            b"open" => Some(Calling::Open),
            b"in" => Some(Calling::In),
            _ => None,
        }
    }
}

#[cfg(test)]
thread_local! {
    // How many times `call` has scanned on this thread, for a test of how often a frame's leading
    // bytes are scanned.
    pub(crate) static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How deep a value skipped ahead of `t` or `id` may nest; one nesting deeper names no call.
const DEPTH: usize = 32;

/// The kind and `id` of the call `bytes`, a frame's first bytes, name: both entries complete at
/// the top level of the frame's map, the kind one that names a call. `None` when either is cut
/// off, comes after an entry that is, is written in a form the scan does not read (an escaped key,
/// an `id` that is no plain unsigned integer), or the frame is no call's.
pub(crate) fn call(codec: Codec, bytes: &[u8]) -> Option<(Calling, u64)> {
    #[cfg(test)]
    SCANS.with(|scans| scans.set(scans.get() + 1));
    let (t, id) = match codec {
        Codec::Json => Json { bytes, at: 0 }.entries()?,
        Codec::Cbor => Cbor { bytes, at: 0 }.entries()?,
        _ => return None,
    };
    Some((Calling::of(t)?, id))
}

struct Json<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Json<'a> {
    fn entries(mut self) -> Option<(&'a [u8], u64)> {
        let (mut t, mut id) = (None, None);
        self.eat(b'{')?;
        loop {
            let key = self.string()?;
            self.eat(b':')?;
            match key {
                b"t" => t = Some(self.string()?),
                b"id" => id = Some(self.unsigned()?),
                _ => self.skip()?,
            }
            if let (Some(t), Some(id)) = (t, id) {
                return Some((t, id));
            }
            self.eat(b',')?;
        }
    }

    fn whitespace(&mut self) {
        while self.bytes.get(self.at).is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        self.whitespace();
        (*self.bytes.get(self.at)? == byte).then(|| self.at += 1)
    }

    /// A string's bytes between its quotes, escapes left as written.
    fn string(&mut self) -> Option<&'a [u8]> {
        self.eat(b'"')?;
        let start = self.at;
        loop {
            match *self.bytes.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(&self.bytes[start..self.at - 1]);
                }
                b'\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
    }

    /// A `u64` written as decimal digits, no leading zero, ended by a byte that is not a digit.
    fn unsigned(&mut self) -> Option<u64> {
        self.whitespace();
        let start = self.at;
        let mut value: u64 = 0;
        while let Some(digit) = self.bytes.get(self.at).filter(|b| b.is_ascii_digit()) {
            value = value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))?;
            self.at += 1;
        }
        let digits = self.at - start;
        let ended = matches!(self.bytes.get(self.at)?, b',' | b'}' | b' ' | b'\t' | b'\n' | b'\r');
        (digits > 0 && ended && (digits == 1 || self.bytes[start] != b'0')).then_some(value)
    }

    /// One value, ended within the bytes.
    fn skip(&mut self) -> Option<()> {
        self.whitespace();
        match *self.bytes.get(self.at)? {
            b'"' => self.string().map(drop),
            b'{' | b'[' => {
                let mut depth = 0;
                loop {
                    match *self.bytes.get(self.at)? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => {
                            depth += 1;
                            if depth > DEPTH {
                                return None;
                            }
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.at += 1;
                                return Some(());
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
            }
            _ => {
                // A number or a literal, ended by the byte after it.
                while !matches!(self.bytes.get(self.at)?, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    self.at += 1;
                }
                Some(())
            }
        }
    }
}

struct Cbor<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// A CBOR item's head: its major type and its argument, `None` for an indefinite length.
struct Head {
    major: u8,
    argument: Option<u64>,
}

/// The byte ending an indefinite-length item.
const BREAK: u8 = 0xff;

impl<'a> Cbor<'a> {
    fn entries(mut self) -> Option<(&'a [u8], u64)> {
        let map = self.head()?;
        if map.major != 5 {
            return None;
        }
        let (mut t, mut id) = (None, None);
        let mut left = map.argument;
        loop {
            match left {
                Some(0) => return None,
                Some(n) => left = Some(n - 1),
                None if *self.bytes.get(self.at)? == BREAK => return None,
                None => {}
            }
            let key = self.text()?;
            match key {
                b"t" => t = Some(self.text()?),
                b"id" => id = Some(self.unsigned()?),
                _ => self.skip(0)?,
            }
            if let (Some(t), Some(id)) = (t, id) {
                return Some((t, id));
            }
        }
    }

    fn head(&mut self) -> Option<Head> {
        let initial = *self.bytes.get(self.at)?;
        self.at += 1;
        let (major, info) = (initial >> 5, initial & 0x1f);
        let width = match info {
            0..=23 => return Some(Head { major, argument: Some(u64::from(info)) }),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            31 if matches!(major, 2..=5) => return Some(Head { major, argument: None }),
            _ => return None,
        };
        let argument = self.bytes.get(self.at..self.at + width)?.iter().fold(0u64, |value, b| (value << 8) | u64::from(*b));
        self.at += width;
        Some(Head { major, argument: Some(argument) })
    }

    /// A definite-length text string's bytes.
    fn text(&mut self) -> Option<&'a [u8]> {
        let head = self.head()?;
        if head.major != 3 {
            return None;
        }
        self.advance(head.argument?)
    }

    fn unsigned(&mut self) -> Option<u64> {
        let head = self.head()?;
        (head.major == 0).then_some(head.argument).flatten()
    }

    fn advance(&mut self, len: u64) -> Option<&'a [u8]> {
        let end = self.at.checked_add(usize::try_from(len).ok()?)?;
        let skipped = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(skipped)
    }

    /// One item, ended within the bytes, nested at `depth`.
    fn skip(&mut self, depth: usize) -> Option<()> {
        if depth > DEPTH {
            return None;
        }
        let head = self.head()?;
        match (head.major, head.argument) {
            (0 | 1 | 7, _) => Some(()),
            (2 | 3, Some(len)) => self.advance(len).map(drop),
            (2 | 3, None) => {
                while *self.bytes.get(self.at)? != BREAK {
                    let chunk = self.head()?;
                    if chunk.major != head.major {
                        return None;
                    }
                    self.advance(chunk.argument?)?;
                }
                self.at += 1;
                Some(())
            }
            (4 | 5, Some(n)) => {
                let items = if head.major == 5 { n.checked_mul(2)? } else { n };
                for _ in 0..items {
                    self.skip(depth + 1)?;
                }
                Some(())
            }
            (4 | 5, None) => {
                while *self.bytes.get(self.at)? != BREAK {
                    self.skip(depth + 1)?;
                    if head.major == 5 {
                        self.skip(depth + 1)?;
                    }
                }
                self.at += 1;
                Some(())
            }
            (6, _) => self.skip(depth + 1),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ulo_rpc::{CallHeaders, Codec, Data, ErrorBody, Frame};
    use ulo_transport::{Details, ErrorKind};

    use super::{Calling, call};

    fn req(codec: Codec, id: u64) -> Frame {
        let mut headers = CallHeaders::new();
        headers.insert("deadline-ms", "500");
        Frame::Req { id, pattern: "orders.place".to_owned(), headers, data: codec.encode("x").expect("a string encodes") }
    }

    /// Every leading cut of `bytes` long enough to hold `t` and `id`, and the whole.
    fn cuts(bytes: &[u8], from: usize) -> impl Iterator<Item = &[u8]> {
        (from..=bytes.len()).map(move |end| &bytes[..end])
    }

    /// `ulo-rpc`'s codec writes `t` and `id` first: every frame naming a call is read from its first
    /// few bytes, whatever follows, and from every longer cut. Planted break: the scan taking `open`
    /// for no call's kind.
    #[test]
    fn the_codec_s_own_frames_name_their_call_in_their_first_bytes() {
        for codec in [Codec::Json, Codec::Cbor] {
            let frames = [
                (req(codec, 7), Calling::Req, 7),
                (Frame::Open { id: u64::MAX, pattern: "orders.stream".to_owned(), headers: CallHeaders::new() }, Calling::Open, u64::MAX),
                (Frame::In { id: 0, data: codec.encode(&[1, 2]).expect("an array encodes") }, Calling::In, 0),
            ];
            for (frame, kind, id) in &frames {
                let bytes = codec.encode_frame(frame).expect("the frame encodes");
                let first = (1..=bytes.len()).find(|end| call(codec, &bytes[..*end]).is_some()).unwrap_or_else(|| panic!("{codec:?} {frame:?} names no call"));
                assert!(first <= 40, "{codec:?} {frame:?} needs {first} bytes");
                for cut in cuts(&bytes, first) {
                    assert_eq!(call(codec, cut), Some((*kind, *id)), "{codec:?} {frame:?} cut at {}", cut.len());
                }
            }
        }
    }

    /// Every cut names the call the whole frame names or none: a number the cut may still continue
    /// is not read, so `"id":12` cut from `"id":123456` names nothing. Planted break: `unsigned`
    /// reading digits the bytes end on.
    #[test]
    fn a_cut_names_the_frame_s_own_call_or_none() {
        for (codec, frame) in [(Codec::Json, &br#"{"t":"req","id":123456,"p":"a"}"#[..]), (Codec::Cbor, &Codec::Cbor.encode_frame(&req(Codec::Cbor, 123_456)).unwrap()[..])] {
            assert_eq!(call(codec, frame), Some((Calling::Req, 123_456)), "{codec:?}");
            for end in 0..frame.len() {
                let named = call(codec, &frame[..end]);
                assert!(named.is_none() || named == Some((Calling::Req, 123_456)), "{codec:?} cut at {end} names {named:?}");
            }
        }
    }

    /// Entries ahead of `t` and `id` are skipped when they end within the bytes, nested values and
    /// strings holding quotes and brackets among them, and a frame whose payload comes first and is
    /// cut off names no call. Planted breaks: a string's escaped quote read as its end; a bracket
    /// inside a string counted as nesting.
    #[test]
    fn entries_ahead_of_the_id_are_skipped_only_when_they_end_within_the_bytes() {
        let ahead = br#"{ "h" : {"k":["v]", "w\"}"]} , "d":[{"x":-1.5e3},true,null,"]"], "p":"a" ,"t":"open","id":42,"d":"#;
        assert_eq!(call(Codec::Json, ahead), Some((Calling::Open, 42)));
        let payload_first = br#"{"d":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"#;
        assert_eq!(call(Codec::Json, payload_first), None);
        let id_after_payload = br#"{"d":"xxxx","t":"req","id":9}"#;
        assert_eq!(call(Codec::Json, id_after_payload), Some((Calling::Req, 9)), "a payload ending within the bytes is skipped");
    }

    /// A CBOR payload ahead of `id` is skipped whatever its shape when it ends within the bytes,
    /// indefinite lengths and tags among them, and names no call when it is cut off. Planted break:
    /// a tag skipped without the item it tags.
    #[test]
    fn a_cbor_payload_ahead_of_the_id_is_skipped_only_when_it_ends_within_the_bytes() {
        // {"d": tag 1 [_ h'01', {_ "k": 1.5}], "t": "in", "id": 300}, then a cut-off text string.
        let mut frame = vec![0xa4, 0x61, b'd', 0xc1, 0x9f, 0x41, 0x01, 0xbf, 0x61, b'k', 0xf9, 0x3e, 0x00, 0xff, 0xff];
        frame.extend([0x61, b't', 0x62, b'i', b'n', 0x62, b'i', b'd', 0x19, 0x01, 0x2c]);
        assert_eq!(call(Codec::Cbor, &frame), Some((Calling::In, 300)));
        let payload_first = [0xa4, 0x61, b'd', 0x7a, 0x00, 0x20, 0x00, 0x00, b'x', b'x'];
        assert_eq!(call(Codec::Cbor, &payload_first), None);
    }

    /// A frame naming no call a server answers names none: an event, a reply, a `cancel`, an `id`
    /// that is negative, fractional or padded, and a key written with an escape, none of which
    /// serde_json reads as the frame's `id` and `t`. Planted break: `unsigned` reading a leading
    /// zero.
    #[test]
    fn a_frame_that_is_no_call_or_an_id_the_scan_does_not_read_names_none() {
        let evt = Frame::Evt { pattern: "a".to_owned(), headers: CallHeaders::new(), data: Data::default() };
        let err = Frame::Err { id: 1, error: ErrorBody::new(ErrorKind::Internal, "x", Details::new()) };
        for codec in [Codec::Json, Codec::Cbor] {
            for frame in [&evt, &err, &Frame::Cancel { id: 1 }] {
                assert_eq!(call(codec, &codec.encode_frame(frame).unwrap()), None, "{codec:?} {frame:?}");
            }
        }
        // The keys `t` and `id` written with a Unicode escape, which a JSON reader takes as `t`
        // and `id`.
        let escaped = format!(r#"{{"{}u0074":"req","id":1}}"#, char::from(0x5c));
        let escaped_id = format!(r#"{{"t":"req","{}u0069d":1}}"#, char::from(0x5c));
        for json in [r#"{"t":"req","id":-1}"#, r#"{"t":"req","id":1.0}"#, r#"{"t":"req","id":01}"#, &escaped, &escaped_id] {
            assert_eq!(call(Codec::Json, json.as_bytes()), None, "{json}");
        }
    }

    /// A value nested past the scan's depth ahead of the `id` names no call, and one at it does.
    /// Planted break: no bound on the depth.
    #[test]
    fn nesting_past_the_depth_names_no_call() {
        let nested = |depth: usize| format!(r#"{{"h":{}{},"t":"req","id":1}}"#, "[".repeat(depth), "]".repeat(depth));
        assert_eq!(call(Codec::Json, nested(super::DEPTH).as_bytes()), Some((Calling::Req, 1)));
        assert_eq!(call(Codec::Json, nested(super::DEPTH + 1).as_bytes()), None);
    }
}
