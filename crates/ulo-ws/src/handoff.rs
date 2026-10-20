//! The gateway hand-off on the HTTP server's port and on an embedding host declaring `upgrades`
//! (transports DESIGN §3.5, X21): the `ulo_http::UpgradeHandler` `WsModule` registers, which finds
//! its gateways through `AppHandle::mounted::<Ws>()` and `::<WsConnect>()` (X20).
//!
//! On the HTTP port, the HTTP server's drain and load shedding answer first. A gateway there is
//! the HTTP server's guest: an upgrade request arriving once the drain has begun, or over the
//! server's `max_inflight`, is answered with the HTTP server's own 503 before the hand-off sees
//! it, so the table's plain-text 503 for a handshake during the drain is a standalone server's.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use http::StatusCode;
use ulo::{AppHandle, BoxError, BoxFuture, DrainToken, Spawn};
use ulo_http::{HttpBody, Request, Response, UpgradeHandler};
use ulo_transport::prepare::Failures;

use crate::connection::{Handshake, Refusal};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults};
use crate::rooms::Hub;
use crate::table::{GatewayDefaults, GatewayTable};
use crate::transport::{Ws, WsConnect};

/// The hand-off itself, carrying `WsModule`'s defaults for the gateways it serves: those whose
/// settings name `Port::Http`, the default.
pub(crate) struct Handoff {
    pub(crate) defaults: GatewayDefaults,
    hub: Arc<Hub>,
    slot: Mutex<Slot>,
}

/// What `prepare` built, and how many HTTP servers hold it. Every HTTP server in the app that
/// prepares, a public port and an admin port for one, calls `prepare` and, as it drops,
/// `release`; one lock holds both, so the last release is the one that drops the table.
#[derive(Default)]
struct Slot {
    /// The gateways on the HTTP server's port, served through the same table a standalone server
    /// serves through. A later `prepare` replaces it, built from the same app.
    table: Option<GatewayTable>,
    holders: usize,
    /// Set by the first `bound`: every holder calls `bound`, and the gateways' `AfterInit` runs
    /// once per app.
    started: bool,
}

impl Handoff {
    pub(crate) fn new(defaults: GatewayDefaults, hub: Arc<Hub>) -> Self {
        Handoff { defaults, hub, slot: Mutex::new(Slot::default()) }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn table(&self) -> Option<GatewayTable> {
        self.slot().table.clone()
    }
}

impl UpgradeHandler for Handoff {
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>> {
        let _ = app;
        self.table().map(|table| table.paths().map(|path| Cow::Owned(path.to_owned())).collect()).unwrap_or_default()
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let table = self.table();
        Box::pin(async move {
            let Some(table) = table.filter(|table| table.gateway_for(req.path()).is_some()) else {
                return plain(StatusCode::NOT_FOUND, "no gateway at this path");
            };
            let Request { head, conn, upgrade, .. } = req;
            let Some(upgrade) = upgrade else {
                return plain(StatusCode::BAD_REQUEST, "this request cannot be upgraded to a WebSocket");
            };
            match table.handshake(head, conn.peer).await {
                Handshake::Switch(switch) => {
                    let response = switch.response().map(|()| HttpBody::empty());
                    switch.serve(upgrade);
                    response
                }
                Handshake::Refuse(refusal) => refusal.into_response().map(HttpBody::from_bytes),
            }
        })
    }

    /// The gateways on the HTTP server's port, built and checked; `WsModule`'s own defaults are
    /// checked for zero limits too, and the broadcast adapter's `prepare` runs beside them. The
    /// calling server holds the hand-off from here, whether this answers `Ok` or `Err`, until its
    /// `release`.
    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        self.slot().holders += 1;
        let connects = app.mounted::<WsConnect>()?;
        let messages = app.mounted::<Ws>()?;
        // `mounted` refuses an app with no runtime, so this finds one.
        let runtime = app.runtime().cloned().ok_or("the WebSocket hand-off was prepared on an app with no runtime")?;
        let mut failures = Failures::new();
        check_defaults("WsModule", &self.defaults, &mut failures);
        if let Err(error) = self.hub.prepare() {
            failures.push_error(error);
        }
        let gateways = build_table(&connects, &messages, Port::Http, &self.defaults, &mut failures);
        let table = GatewayTable::new(gateways, Arc::clone(&self.hub), app.clone(), runtime, &self.defaults);
        self.slot().table = Some(table);
        failures.into_result()
    }

    /// Starts broadcast delivery and each gateway's `AfterInit` on its own task, at the first
    /// holder's `bound` only.
    fn bound(&self, app: &AppHandle) {
        let _ = app;
        let table = {
            let mut slot = self.slot();
            let table = slot.table.clone().filter(|_| !slot.started);
            slot.started |= table.is_some();
            table
        };
        if let Some(table) = table {
            table.start();
        }
    }

    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        let table = self.table();
        Box::pin(async move {
            if let Some(table) = table {
                table.drain(token).await;
            }
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        let table = self.table();
        Box::pin(async move {
            match table {
                Some(table) => table.close().await,
                None => self.hub.stop(),
            }
        })
    }

    /// Drops the table `prepare` built once the last server holding it releases: the table holds
    /// the app's `AppHandle` and the hand-off sits in the app's graph, so the table would otherwise
    /// keep the app, and the hub with it. A server releasing while another still serves the
    /// gateways leaves the table to it.
    fn release(&self) {
        let table = {
            let mut slot = self.slot();
            slot.holders = slot.holders.saturating_sub(1);
            if slot.holders == 0 { slot.table.take() } else { None }
        };
        // Dropped after the lock is released: the table's drop reaches the app's shared state.
        drop(table);
    }
}

/// Each gateway's `AfterInit`, spawned on `runtime` and detached: the server does not wait for
/// it. A failure is logged.
pub(crate) fn after_init(gateways: &[Arc<GatewayRuntime>], runtime: &dyn Spawn) {
    for gateway in gateways {
        let Some(hook) = gateway.handler.after_init.clone() else { continue };
        let module = gateway.connect.module().clone();
        let reference = gateway.reference();
        let path = Arc::clone(&gateway.path);
        drop(runtime.spawn(Box::pin(async move {
            if let Err(error) = hook(module, reference).await {
                tracing::warn!(%error, gateway = %path, "after_init failed");
            }
        })));
    }
}

/// A refusal written as the handshake's are.
fn plain(status: StatusCode, reason: &str) -> Response {
    Refusal::new(status, reason).into_response().map(HttpBody::from_bytes)
}
