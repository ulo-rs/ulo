//! The broadcast adapter SPI, written without macros, and the in-memory adapter (transports
//! DESIGN §4.3).
//!
//! An adapter carries a frame between the processes of one application: `publish` sends it to
//! every process, and each process's `subscribe` stream delivers it to that process's own members
//! of the target. Room membership stays local to each process. Delivery is best effort and
//! ordered per sender within one process.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_core::stream::BoxStream;
use async_broadcast::{InactiveReceiver, RecvError, Sender};
use serde::{Deserialize, Serialize};
use ulo::{BoxError, BoxFuture, Timer};

use crate::connection::ConnId;

/// The broadcast SPI: an in-memory adapter for one process, `ulo_ws_redis::Redis` across
/// processes. Set on the module, `WsModule::for_root().broadcast(adapter)`; the module binds it
/// as `dyn BroadcastAdapter`.
pub trait BroadcastAdapter: Send + Sync + 'static {
    /// What the adapter refuses before any I/O: on an adapter tied to a tokio runtime, being built
    /// outside one and given none. `WsModule`'s init hook calls it, so the refusal fails `connect`
    /// in a process that serves no gateway too, and a server serving gateways calls it again from
    /// its own `prepare`, the hand-off on the HTTP server's port and a standalone server alike, a
    /// check that refuses nothing it did not refuse at `connect`. Unset, it refuses nothing, as the
    /// in-memory adapter.
    fn prepare(&self) -> Result<(), BoxError> {
        Ok(())
    }

    /// Sends `frame`, an encoded envelope, to every process's members of `target`.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>>;

    /// What every process published, this one's included, for the process `node` to deliver to
    /// its own members. The future resolves once the subscription is live, so that a broadcast
    /// published after it resolves reaches the stream; an adapter whose subscription Redis or a
    /// broker confirms waits for the confirmation, and fails the future, which fails the server's
    /// `listen()` as `Bind`, when none comes within the adapter's connect bound.
    ///
    /// `timer` is the app's clock. An adapter times its connect bound and its pacing on it, through
    /// `ulo::timeout` for a bound whose expiry is reported, so the wait follows the app's `Timer`
    /// and not a clock of the adapter's own runtime. A clock that refuses, its runtime having shut
    /// down, ends the subscription as `Permanent` `RuntimeShutDown`.
    fn subscribe(&self, node: NodeId, timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>>;
}

/// Who a broadcast reaches. Between processes it travels in the form [`wire`] defines.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The gateway path or namespace addressed; `None` addresses every gateway.
    pub gateway: Option<String>,
    pub audience: Audience,
    /// Connections left out.
    pub except: Vec<ConnId>,
}

impl Target {
    pub fn new(gateway: Option<String>, audience: Audience, except: Vec<ConnId>) -> Self {
        Target { gateway, audience, except }
    }
}

/// The members of a broadcast within its gateway or gateways.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    All,
    Room(String),
    Client(ConnId),
}

/// One process among those sharing an adapter: random per process, part of every `ConnId` it
/// hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub(crate) u128);

impl NodeId {
    /// This process's id, the same for its whole life.
    ///
    /// Drawn from the standard library's per-process random hash keys, the process id and the
    /// clock, so two processes started together on one host still differ.
    pub fn current() -> NodeId {
        static CURRENT: OnceLock<NodeId> = OnceLock::new();
        *CURRENT.get_or_init(|| {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
            let half = |salt: u64| {
                let mut hasher = RandomState::new().build_hasher();
                hasher.write_u64(salt);
                hasher.write_u32(std::process::id());
                hasher.write_u128(nanos);
                hasher.finish()
            };
            NodeId((u128::from(half(0)) << 64) | u128::from(half(1)))
        })
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The adapter for one process, `WsModule::for_root()`'s unless `.broadcast(..)` names another.
///
/// A clone shares the channel. A subscriber more than 1024 broadcasts behind skips the ones it
/// missed, logged at `warn`: delivery is best effort.
#[derive(Clone, Default)]
pub struct InMemory {
    inner: Arc<Channel>,
}

/// `async-broadcast` in overflow mode: the sender never waits, and a full queue drops its oldest
/// broadcast, which a subscriber that had not read it learns as `Overflowed`.
struct Channel {
    sender: Sender<(Target, Bytes)>,
    /// Keeps the channel open while no subscriber is active; it holds no broadcast.
    _open: InactiveReceiver<(Target, Bytes)>,
}

impl Default for Channel {
    fn default() -> Self {
        let (mut sender, receiver) = async_broadcast::broadcast(1024);
        sender.set_overflow(true);
        Channel { sender, _open: receiver.deactivate() }
    }
}

impl InMemory {
    pub fn new() -> Self {
        InMemory::default()
    }
}

impl BroadcastAdapter for InMemory {
    /// With no subscriber there is no member to deliver to, which is not a failure.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        // `Inactive` when no subscriber is active. In overflow mode a full queue drops its oldest
        // broadcast rather than refusing this one.
        let _ = self.inner.sender.try_broadcast((target, frame));
        Box::pin(async { Ok(()) })
    }

    fn subscribe(&self, node: NodeId, timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>> {
        let _ = (node, timer);
        // Starts at the next broadcast, as a subscriber joining late should. The receiver exists
        // when this returns, so the subscription is live before the future is polled.
        let receiver = self.inner.sender.new_receiver();
        let carried: BoxStream<'static, (Target, Bytes)> = Box::pin(futures_util::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv_direct().await {
                    Ok(item) => return Some((item, receiver)),
                    Err(RecvError::Overflowed(missed)) => {
                        tracing::warn!(missed, "a WebSocket broadcast subscriber fell behind and skipped broadcasts");
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        }));
        Box::pin(async move { Ok(carried) })
    }
}

pub mod wire {
    //! The form a broadcast takes between processes: what an adapter carrying broadcasts across
    //! processes publishes with [`encode`] and reads back with [`decode`].
    //!
    //! A message is one version byte, [`VERSION`], the [`Target`] as one line of JSON, a newline,
    //! then the encoded broadcast:
    //!
    //! ```text
    //! 0x01 {"gateway":null,"audience":{"kind":"room","room":"lobby"},"except":[]} 0x0A <frame>
    //! ```
    //!
    //! `gateway` is the path or namespace addressed, or `null` for every gateway. `audience` is
    //! `{"kind":"all"}`, `{"kind":"room","room":<name>}` or `{"kind":"client","client":<id>}`.
    //! `except` lists ids, each `{"node":<32 lowercase hex digits>,"seq":<integer>}`. A field this
    //! version does not name makes the message [`Undecodable::Malformed`].
    //!
    //! The version byte is read before anything is parsed, so a later version may change the whole
    //! layout and a reader of this one still tells it from a corrupt message. Any change to the
    //! layout takes a new version. Versions count up from 1 and stop short of `0x7B`, `{`, which
    //! opens the unversioned layout: the target's JSON in the shape its derived `serde` impls
    //! wrote, a newline and the frame, with no version byte. A reader refuses that layout, another
    //! version and an audience kind it does not know, each with its own [`Undecodable`], and the
    //! adapter drops the message.
    //!
    //! ```
    //! use ulo_ws::wire::{self, Undecodable};
    //! use ulo_ws::{Audience, Target};
    //!
    //! let target = Target::new(None, Audience::Room("lobby".into()), Vec::new());
    //! let message = wire::encode(&target, br#"{"event":"hi"}"#);
    //! assert_eq!(message[0], wire::VERSION);
    //! let (read, frame) = wire::decode(&message).expect("a message this version wrote");
    //! assert_eq!((read, &frame[..]), (target, &br#"{"event":"hi"}"#[..]));
    //!
    //! assert_eq!(wire::decode(b"\x02{}\n"), Err(Undecodable::Version(2)));
    //! ```

    use std::fmt;

    use bytes::Bytes;
    use serde::{Deserialize, Serialize};
    use serde_json::value::RawValue;

    use super::{Audience, NodeId, Target};
    use crate::connection::ConnId;

    /// The version [`encode`] writes and the one [`decode`] reads.
    pub const VERSION: u8 = 1;

    /// The first byte of the unversioned layout, the opening brace of its JSON.
    const UNVERSIONED: u8 = b'{';

    /// `frame` addressed to `target`, as a message of [`VERSION`].
    pub fn encode(target: &Target, frame: &[u8]) -> Vec<u8> {
        // Inside its own crate `Audience` is matched with no wildcard, so a new variant does not
        // compile until the wire names it, under a new version.
        let audience = match &target.audience {
            Audience::All => Kind::All {},
            Audience::Room(room) => Kind::Room { room: room.clone() },
            Audience::Client(id) => Kind::Client { client: Id::from(*id) },
        };
        let header = HeaderOut {
            gateway: target.gateway.as_deref(),
            audience,
            except: target.except.iter().copied().map(Id::from).collect(),
        };
        let mut message = vec![VERSION];
        serde_json::to_writer(&mut message, &header).expect("a header of strings and integers serializes into a `Vec`");
        message.push(b'\n');
        message.extend_from_slice(frame);
        message
    }

    /// The target and frame of `message`, or why this version cannot read it.
    pub fn decode(message: &[u8]) -> Result<(Target, Bytes), Undecodable> {
        let (&version, rest) = message.split_first().ok_or_else(|| Undecodable::Malformed("the message is empty".to_owned()))?;
        match version {
            VERSION => {}
            UNVERSIONED => return Err(Undecodable::Unversioned),
            other => return Err(Undecodable::Version(other)),
        }
        // Compact JSON writes a newline inside a string as `\n`, so the first newline ends the header.
        let split = rest.iter().position(|&byte| byte == b'\n').ok_or_else(|| Undecodable::Malformed("no newline ends the target".to_owned()))?;
        let malformed = |error: serde_json::Error| Undecodable::Malformed(error.to_string());
        let header: HeaderIn = serde_json::from_slice(&rest[..split]).map_err(malformed)?;
        let named: Named = serde_json::from_str(header.audience.get()).map_err(malformed)?;
        if !KINDS.contains(&named.kind.as_str()) {
            return Err(Undecodable::Audience(named.kind));
        }
        let audience = match serde_json::from_str(header.audience.get()).map_err(malformed)? {
            Kind::All {} => Audience::All,
            Kind::Room { room } => Audience::Room(room),
            Kind::Client { client } => Audience::Client(client.conn()?),
        };
        let except = header.except.into_iter().map(Id::conn).collect::<Result<_, _>>()?;
        Ok((Target::new(header.gateway, audience, except), Bytes::copy_from_slice(&rest[split + 1..])))
    }

    /// Why [`decode`] refused a message.
    #[non_exhaustive]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Undecodable {
        /// The unversioned layout, opening with the target's JSON where the version byte stands.
        Unversioned,
        /// A version other than [`VERSION`].
        Version(u8),
        /// An audience kind this version does not know, as the message names it.
        Audience(String),
        /// A message of [`VERSION`] that does not parse, and why.
        Malformed(String),
    }

    impl fmt::Display for Undecodable {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Undecodable::Unversioned => f.write_str("the broadcast is in the unversioned wire layout"),
                Undecodable::Version(version) => write!(f, "the broadcast is wire version {version}; this build reads version {VERSION}"),
                Undecodable::Audience(kind) => write!(f, "the broadcast addresses an audience of kind `{kind}`, which wire version {VERSION} does not name"),
                Undecodable::Malformed(reason) => write!(f, "the broadcast does not parse as wire version {VERSION}: {reason}"),
            }
        }
    }

    impl std::error::Error for Undecodable {}

    /// The audience kinds of this version, as `Kind` names them on the wire.
    const KINDS: [&str; 3] = ["all", "room", "client"];

    #[derive(Serialize)]
    struct HeaderOut<'a> {
        gateway: Option<&'a str>,
        audience: Kind,
        except: Vec<Id>,
    }

    /// The audience stays raw until its kind is read, so an unknown kind is told from a malformed one.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct HeaderIn {
        gateway: Option<String>,
        audience: Box<RawValue>,
        except: Vec<Id>,
    }

    #[derive(Deserialize)]
    struct Named {
        kind: String,
    }

    /// `All` is a struct variant: serde ignores the other fields beside an internally tagged unit
    /// variant's tag, where it refuses them beside a struct variant's.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
    enum Kind {
        All {},
        Room { room: String },
        Client { client: Id },
    }

    /// A [`ConnId`]; the node as hex digits, which a JSON reader holding numbers as doubles keeps
    /// exact where a 128-bit number would not survive.
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Id {
        node: String,
        seq: u64,
    }

    impl From<ConnId> for Id {
        fn from(id: ConnId) -> Self {
            Id { node: id.node.to_string(), seq: id.seq }
        }
    }

    impl Id {
        fn conn(self) -> Result<ConnId, Undecodable> {
            let hex = self.node.len() == 32 && self.node.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
            match u128::from_str_radix(&self.node, 16) {
                Ok(node) if hex => Ok(ConnId { node: NodeId(node), seq: self.seq }),
                _ => Err(Undecodable::Malformed(format!("`{}` is not a node id of 32 lowercase hex digits", self.node))),
            }
        }
    }
}
