use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use ulo::{App, AppBuilder, BoxError, Dep, Ext, Guard, injectable, module, routes};
use ulo_ws::{
    ConnectCx, ConnectRefused, Connection, DisconnectReason, OnConnect, OnDisconnect, Payload, UpgradeHead, Session, Ws, WsCx, WsModule,
};

mod badge;
pub use badge::{Admit, BADGE, Badge};

/// The header a visitor names itself in on the upgrade request.
pub const NAME: &str = "x-name";
pub const DRAIN: Duration = Duration::from_secs(5);

/// The connection's session: the name its upgrade carried.
pub struct Visitor {
    pub name: String,
    /// Set once by `on_connect` from the connect guard's `Badge`.
    pub badge: OnceLock<String>,
}

/// Written into each message's extensions by the message guard, read by the handler as `Ext<Speaker>`.
pub struct Speaker(pub String);

/// Every connect and disconnect the gateway saw, in the order they happened.
#[derive(Default)]
pub struct Journal(Mutex<Vec<String>>);

#[injectable]
impl Journal {
    fn new() -> Self {
        Journal::default()
    }
}

impl Journal {
    pub fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn record(&self, entry: String) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
    }
}

/// Runs before every message and names who sent it.
struct Stamp;

impl Guard<Ws> for Stamp {
    async fn can_activate(&self, cx: &WsCx) -> Result<bool, BoxError> {
        let name = cx.conn().session().get::<Visitor>().map_or_else(|| "nobody".to_owned(), |visitor| visitor.name.clone());
        cx.extensions().insert(Speaker(name));
        Ok(true)
    }
}

#[injectable]
pub struct Chat {
    journal: Dep<Journal>,
}

#[routes]
#[ulo_ws::gateway(
    path = "/chat",
    port = own,
    session_with = |head: Dep<UpgradeHead>| Visitor {
        name: head.headers().get(NAME).and_then(|name| name.to_str().ok()).unwrap_or("anonymous").to_owned(),
        badge: OnceLock::new(),
    },
    connect_guards(value = Admit),
)]
#[guards(value = Stamp)]
impl Chat {
    /// Answers the message that carried the same `id`.
    #[ulo_ws::message("say")]
    fn say(&self, text: Payload<String>, speaker: Ext<Speaker>) -> String {
        format!("{}: {}", speaker.0, text.0)
    }

    /// The badge the connect guard found, as `on_connect` kept it.
    #[ulo_ws::message("badge")]
    fn badge(&self, visitor: Session<Visitor>) -> String {
        visitor.badge.get().cloned().unwrap_or_else(|| "unset".to_owned())
    }
}

impl OnConnect for Chat {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        let name = cx.conn().session().get::<Visitor>().map_or_else(|| "nobody".to_owned(), |visitor| visitor.name.clone());
        self.journal.record(format!("connected {name}"));
        let badge = cx.ext::<Badge>().map_or_else(|_| "missing".to_owned(), |badge| badge.0.clone());
        if let Some(visitor) = cx.conn().session().get::<Visitor>() {
            let _ = visitor.badge.set(badge);
        }
        Ok(())
    }
}

impl OnDisconnect for Chat {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        let name = conn.session().get::<Visitor>().map_or_else(|| "nobody".to_owned(), |visitor| visitor.name.clone());
        self.journal.record(format!("disconnected {name}: {why:?}"));
    }
}

#[module(imports = [WsModule::for_root()], providers = [Journal], controllers = [Chat])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule).drain_timeout(DRAIN)
}
