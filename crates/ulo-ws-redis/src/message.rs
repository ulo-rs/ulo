use serde::{Deserialize, Serialize};
use ulo::ws::WsMessage;

/// Envelope published to `ulo:broadcast`, and to a process's own `ulo:broadcast:{process_id}`
/// channel for `to_client`.
///
/// Every broadcast call serializes to this format, and every subscriber
/// deserializes from it to deliver messages to locally connected clients.
#[derive(Serialize, Deserialize)]
pub(crate) struct RedisBroadcastPayload {
    pub target: BroadcastTargetKind,
    pub namespace: Option<String>,
    pub message: WsMessage,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) enum BroadcastTargetKind {
    All,
    Room(String),
    Rooms(Vec<String>),
    Client(String),
    Except(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(target: BroadcastTargetKind, namespace: Option<&str>, message: WsMessage) -> String {
        serde_json::to_string(&RedisBroadcastPayload {
            target,
            namespace: namespace.map(String::from),
            message,
        })
        .unwrap()
    }

    /// The bytes on either broadcast channel are serde's derived rendering of the payload: a field
    /// map whose `target` and `message` are externally tagged enums. A process on the other side of
    /// a rolling deploy parses exactly these, and a `#[serde(rename)]` anywhere in the three types
    /// changes them.
    #[test]
    fn the_envelope_is_the_derived_shape() {
        assert_eq!(
            json(BroadcastTargetKind::All, None, WsMessage::text("hi")),
            r#"{"target":"All","namespace":null,"message":{"Text":"hi"}}"#
        );
        assert_eq!(
            json(
                BroadcastTargetKind::Room("lobby".into()),
                Some("chat"),
                WsMessage::binary(vec![0, 1, 2, 250])
            ),
            r#"{"target":{"Room":"lobby"},"namespace":"chat","message":{"Binary":[0,1,2,250]}}"#
        );
        assert_eq!(
            json(
                BroadcastTargetKind::Rooms(vec!["a".into(), "b".into()]),
                None,
                WsMessage::close_with(1008, "nope")
            ),
            r#"{"target":{"Rooms":["a","b"]},"namespace":null,"message":{"Close":{"code":1008,"reason":"nope"}}}"#
        );
        assert_eq!(
            json(
                BroadcastTargetKind::Client("c1".into()),
                None,
                WsMessage::close()
            ),
            r#"{"target":{"Client":"c1"},"namespace":null,"message":{"Close":null}}"#
        );
        assert_eq!(
            json(
                BroadcastTargetKind::Except("c1".into()),
                None,
                WsMessage::Ping(vec![1, 2])
            ),
            r#"{"target":{"Except":"c1"},"namespace":null,"message":{"Ping":[1,2]}}"#
        );
        assert_eq!(
            json(BroadcastTargetKind::All, None, WsMessage::Pong(vec![3])),
            r#"{"target":"All","namespace":null,"message":{"Pong":[3]}}"#
        );
    }

    /// A payload naming a variant this build does not know fails to parse, and the subscriber drops
    /// what fails to parse. A variant added to `WsMessage` reaches an older process as exactly this.
    #[test]
    fn an_unknown_message_variant_does_not_parse() {
        let err = match serde_json::from_str::<RedisBroadcastPayload>(
            r#"{"target":"All","namespace":null,"message":{"Compressed":"x"}}"#,
        ) {
            Ok(_) => panic!("a payload naming an unknown variant parsed"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("unknown variant"), "{err}");
    }
}
