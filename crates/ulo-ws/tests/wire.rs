//! The bytes of the cross-process broadcast wire, one literal per audience kind, per gateway form
//! and per `except` form, so a change to the layout fails here rather than between two processes of
//! a rolling deploy; and what a reader of this version refuses, each refusal by its variant.
//!
//! A `ConnId` has no public constructor, so the client and `except` cases read their ids off the
//! golden bytes and require that encoding them gives back those bytes.

use ulo_ws::wire::{self, Undecodable};
use ulo_ws::{Audience, Target};

/// A frame holding a newline and bytes that are not UTF-8: only the first newline ends the target.
const FRAME: &[u8] = b"\x00\n\xff{\"event\":\"x\"}";

fn with_frame(header: &[u8]) -> Vec<u8> {
    let mut message = header.to_vec();
    message.extend_from_slice(FRAME);
    message
}

fn decoded(message: &[u8]) -> Target {
    let (target, frame) = wire::decode(message).unwrap_or_else(|refused| panic!("{:?} did not decode: {refused}", String::from_utf8_lossy(message)));
    assert_eq!(&frame[..], FRAME, "the frame after the target");
    target
}

#[test]
fn everyone_on_every_gateway() {
    let golden = with_frame(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[]}\n");
    let target = Target::new(None, Audience::All, Vec::new());
    assert_eq!(wire::encode(&target, FRAME), golden);
    assert_eq!(decoded(&golden), target);
}

#[test]
fn a_room_on_one_gateway() {
    let golden = with_frame(b"\x01{\"gateway\":\"/chat\",\"audience\":{\"kind\":\"room\",\"room\":\"lobby\"},\"except\":[]}\n");
    let target = Target::new(Some("/chat".to_owned()), Audience::Room("lobby".to_owned()), Vec::new());
    assert_eq!(wire::encode(&target, FRAME), golden);
    assert_eq!(decoded(&golden), target);
}

#[test]
fn a_room_name_holding_a_newline_is_escaped_inside_the_target() {
    let golden = with_frame(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"room\",\"room\":\"two\\nlines\"},\"except\":[]}\n");
    let target = Target::new(None, Audience::Room("two\nlines".to_owned()), Vec::new());
    assert_eq!(wire::encode(&target, FRAME), golden);
    assert_eq!(decoded(&golden), target);
}

#[test]
fn one_client() {
    let golden = with_frame(
        b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"client\",\"client\":{\"node\":\"000102030405060708090a0b0c0d0e0f\",\"seq\":7}},\"except\":[]}\n",
    );
    let target = decoded(&golden);
    let Audience::Client(id) = &target.audience else { panic!("a client audience decoded as {:?}", target.audience) };
    assert_eq!(id.to_string(), "000102030405060708090a0b0c0d0e0f-7");
    assert_eq!(id.node().to_string(), "000102030405060708090a0b0c0d0e0f");
    assert_eq!((target.gateway.as_deref(), target.except.as_slice()), (None, &[][..]));
    assert_eq!(wire::encode(&target, FRAME), golden);
}

#[test]
fn everyone_except_two_connections() {
    let golden = with_frame(
        b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[{\"node\":\"ffffffffffffffffffffffffffffffff\",\"seq\":0},{\"node\":\"00000000000000000000000000000001\",\"seq\":18446744073709551615}]}\n",
    );
    let target = decoded(&golden);
    assert_eq!(target.audience, Audience::All);
    let except: Vec<String> = target.except.iter().map(ToString::to_string).collect();
    assert_eq!(except, ["ffffffffffffffffffffffffffffffff-0", "00000000000000000000000000000001-18446744073709551615"]);
    assert_eq!(wire::encode(&target, FRAME), golden);
}

#[test]
fn an_empty_frame_ends_at_the_newline() {
    let target = Target::new(None, Audience::All, Vec::new());
    let message = wire::encode(&target, b"");
    assert_eq!(message, b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[]}\n");
    assert_eq!(wire::decode(&message).map(|(_, frame)| frame.len()), Ok(0));
}

/// The layout `Target`'s derived `serde` impls wrote, with no version byte.
#[test]
fn the_unversioned_layout_is_refused() {
    let everyone = b"{\"gateway\":null,\"audience\":\"All\",\"except\":[]}\n{\"event\":\"x\",\"data\":1}";
    let room = b"{\"gateway\":null,\"audience\":{\"Room\":\"lobby\"},\"except\":[]}\n{\"event\":\"x\",\"data\":1}";
    assert_eq!(wire::decode(everyone), Err(Undecodable::Unversioned));
    assert_eq!(wire::decode(room), Err(Undecodable::Unversioned));
}

#[test]
fn another_version_is_refused_by_its_number_whatever_follows() {
    for version in [0u8, 2, 0x7A, 0x7C, 0xFF] {
        let mut message = vec![version];
        message.extend_from_slice(b"\x00 not JSON at all");
        assert_eq!(wire::decode(&message), Err(Undecodable::Version(version)), "version {version}");
    }
}

#[test]
fn an_unknown_audience_kind_is_refused_by_its_name() {
    let message = with_frame(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"everyone_but\",\"rooms\":[\"a\"]},\"except\":[]}\n");
    assert_eq!(wire::decode(&message), Err(Undecodable::Audience("everyone_but".to_owned())));
}

fn malformed(message: &[u8], names: &str) {
    match wire::decode(message) {
        Err(Undecodable::Malformed(reason)) => assert!(reason.contains(names), "the reason {reason:?} does not name {names:?}"),
        other => panic!("{:?} decoded as {other:?}, not as malformed", String::from_utf8_lossy(message)),
    }
}

#[test]
fn an_empty_message_is_malformed() {
    malformed(b"", "empty");
}

#[test]
fn a_target_with_no_newline_is_malformed() {
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[]}", "newline");
}

#[test]
fn a_field_this_version_does_not_name_is_malformed() {
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[],\"rooms\":[]}\n", "rooms");
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"room\",\"room\":\"a\",\"also\":\"b\"},\"except\":[]}\n", "also");
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\",\"rooms\":[]},\"except\":[]}\n", "rooms");
    malformed(
        b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"},\"except\":[{\"node\":\"00000000000000000000000000000001\",\"seq\":1,\"at\":2}]}\n",
        "at",
    );
}

#[test]
fn a_missing_field_is_malformed() {
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"kind\":\"all\"}}\n", "except");
    malformed(b"\x01{\"gateway\":null,\"audience\":{\"room\":\"a\"},\"except\":[]}\n", "kind");
    malformed(b"\x01{\"gateway\":null,\"audience\":\"all\",\"except\":[]}\n", "");
}

#[test]
fn a_node_id_that_is_not_32_hex_digits_is_malformed() {
    for node in ["0000000000000000000000000000001", "+0000000000000000000000000000001", "0000000000000000000000000000000g", "1", "0000000000000000000000000000000A"] {
        let message = format!("\x01{{\"gateway\":null,\"audience\":{{\"kind\":\"client\",\"client\":{{\"node\":\"{node}\",\"seq\":1}}}},\"except\":[]}}\n");
        malformed(message.as_bytes(), node);
    }
}
