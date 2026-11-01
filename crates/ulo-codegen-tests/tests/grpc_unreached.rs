//! A scope of a `PreDispatch<Grpc>` entry that covers no method of the service table is refused by
//! `listen()`, as it is on HTTP: per scope, with exclusions taken out first, naming the line that
//! wrote the entry. The check runs once in the shared stage for both transports (X23).

mod support;
mod wire;

use std::sync::atomic::{AtomicU32, Ordering};

use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, PrepareEntry, PrepareError, StartupError, TypeName, injectable};
use ulo_grpc::Grpc;
use ulo_http::{Middleware, Request, Response};

use support::within;
use wire::Hold;

/// Passes every request through.
#[injectable]
struct Pass;

impl Middleware for Pass {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        next.run(req).await
    }
}

static MIXED: AtomicU32 = AtomicU32::new(0);
static EXCLUDED: AtomicU32 = AtomicU32::new(0);
static COVERING: AtomicU32 = AtomicU32::new(0);

macro_rules! declared_at {
    ($line:expr, $declaration:expr) => {{
        $line.store(line!(), Ordering::SeqCst);
        $declaration;
    }};
}

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Hold::new());
        m.controller::<wire::Service>();
        m.provide::<Pass>();
        (self.0)(m);
    }
}

fn one_scope_covers_and_one_does_not(m: &mut ModuleDef<'_>) {
    declared_at!(COVERING, m.meta::<ulo_grpc::PreDispatch>().apply_for::<Pass>(["/codegen.v1.Counter/*"]));
    declared_at!(MIXED, m.meta::<ulo_grpc::PreDispatch>().apply_for::<Pass>(["/codegen.v1.Counter/*", "/typo"]));
}

fn every_covered_method_excluded(m: &mut ModuleDef<'_>) {
    // One line: the entry's location is the line of the `apply_for` call, and the macro records its own.
    #[rustfmt::skip]
    declared_at!(EXCLUDED, m.meta::<ulo_grpc::PreDispatch>().apply_for::<Pass>(["/codegen.v1.Counter/Add"]).exclude(["/codegen.v1.Counter/Add"]));
}

async fn entries(register: fn(&mut ModuleDef<'_>)) -> Vec<PrepareEntry> {
    let bound = App::builder(Root(register))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false));
    let Err(error) = within("listen", bound.listen()).await else {
        panic!("an entry covering no method listened");
    };
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let refused = errors.iter().next().expect("one entry");
    assert_eq!(refused.transport, TypeName::of::<Grpc>());
    refused.source.downcast_ref::<PrepareError>().expect("a PrepareError").entries().to_vec()
}

fn is_unreached(entry: &PrepareEntry, line: &AtomicU32, scope: &str) -> bool {
    let line = line.load(Ordering::SeqCst);
    assert_ne!(line, 0, "the declaration never ran");
    entry.location().is_some_and(|at| at.file() == file!() && at.line() == line)
        && entry.subject() == Some("pre-dispatch entry")
        && entry.scopes() == [scope.to_owned()]
        && entry.names().is_empty()
}

#[tokio::test]
async fn a_scope_covering_no_method_is_refused_beside_one_that_does() {
    let entries = entries(one_scope_covers_and_one_does_not).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(is_unreached(&entries[0], &MIXED, "/typo"), "{entries:?}");
}

#[tokio::test]
async fn a_scope_whose_covered_methods_are_all_excluded_is_refused() {
    let entries = entries(every_covered_method_excluded).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(is_unreached(&entries[0], &EXCLUDED, "/codegen.v1.Counter/Add"), "{entries:?}");
}
