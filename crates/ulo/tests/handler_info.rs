//! `dispatch` sets the mounted handler's `HandlerInfo` on the execution before the first guard
//! runs (transports DESIGN §2.5, X3), so a guard reads the declared metadata of the handler it
//! guards, and so does an error handler offered a guard's refusal.
//!
//! The metadata type, route and name are chosen to differ between the two handlers: a guard that
//! saw the wrong handler's info, or none, writes a different line.

#[macro_use]
mod common;

use std::sync::Arc;

use common::pipeline::*;
use common::*;
use ulo::scope::Singleton;
use ulo::{AnyGuard, BoxError, EnhancerSpec, ErrorHandler, Guard, HandlerSpec, Metadata};

struct A;

#[derive(Debug, PartialEq)]
struct RequiresRole(&'static str);

/// What a guard or error handler reads off the execution's handler, as one line.
fn seen(cx: &PCx) -> String {
    match cx.exec.handler() {
        None => "nothing".to_owned(),
        Some(info) => format!(
            "{} {} {:?} {:?} {:?}",
            info.transport(),
            info.name(),
            info.route(),
            info.meta::<RequiresRole>(),
            info.meta_all::<RequiresRole>().collect::<Vec<_>>()
        ),
    }
}

/// Writes `<name> sees <line>` and answers `admit`.
struct Reads {
    name: &'static str,
    admit: bool,
}

impl Guard<TP> for Reads {
    async fn can_activate(&self, cx: &PCx) -> Result<bool, BoxError> {
        cx.log.push(format!("{} sees {}", self.name, seen(cx)));
        Ok(self.admit)
    }
}

/// Writes `handle sees <line>` and hands the error on.
struct ReadsOnError;

impl ErrorHandler<TP> for ReadsOnError {
    async fn handle(&self, err: BoxError, cx: &PCx) -> Result<String, BoxError> {
        cx.log.push(format!("handle sees {}", seen(cx)));
        Err(err)
    }
}

typed_controller!(Declared, Singleton, |m| {
    let mut controller = EnhancerSpec::<TP>::new();
    controller.guard_value(Reads { name: "c", admit: true });
    let mut method = EnhancerSpec::<TP>::new();
    method.guard_value(Reads { name: "m", admit: true });
    let mut meta = Metadata::new();
    meta.controller(RequiresRole("admin")).method(RequiresRole("owner"));
    m.handler::<TP, _>(HandlerSpec::new("things", ()).route("/things").meta(meta).controller(controller).method(method));

    let mut refusing = EnhancerSpec::<TP>::new();
    refusing.guard_value(Reads { name: "r", admit: false }).error_handler_value(ReadsOnError);
    let mut meta = Metadata::new();
    meta.controller(RequiresRole("auditor"));
    m.handler::<TP, _>(HandlerSpec::new("audit", ()).route("/audit").meta(meta).method(refusing));
});

async fn declared(log: &Log) -> ulo::App<ulo::Connected> {
    let l = log.clone();
    connected_on_tokio(Mod::<A>::new(move |m| {
        bind_log(m, &l);
        m.enhancer::<AnyGuard<TP>>().value(Arc::new(Reads { name: "g", admit: true }));
        m.controller::<Declared>();
    }))
    .await
}

fn sees(log: &Log) -> Vec<String> {
    log.events().into_iter().filter(|line| line.contains(" sees ")).collect()
}

#[tokio::test]
async fn the_first_guard_reads_the_metadata_of_the_handler_it_guards() {
    let log = Log::new();
    let app = declared(&log).await;
    let handler = named::<TP>(&app, "things");
    within("call", call(&handler, &log, Body::Ok)).await.expect("admits");

    // The global guard runs before any other enhancer, so it is the first reader; the method's
    // declaration wins `meta` and `meta_all` lists the method's first.
    let line = r#"p things Some("/things") Some(RequiresRole("owner")) [RequiresRole("owner"), RequiresRole("admin")]"#;
    assert_eq!(sees(&log), vec![format!("g sees {line}"), format!("c sees {line}"), format!("m sees {line}")]);
    drop(handler);
    close(app).await;
}

#[tokio::test]
async fn a_refused_call_has_its_handler_set_for_the_guard_and_the_error_handler_alike() {
    let log = Log::new();
    let app = declared(&log).await;
    let handler = named::<TP>(&app, "audit");
    within("call", call(&handler, &log, Body::Ok)).await.expect_err("the guard refuses");

    // Nothing was admitted, so a `HandlerInfo` set after admission would never exist for either
    // reader. Each reads this handler's own declaration, not the other handler's.
    let line = r#"p audit Some("/audit") Some(RequiresRole("auditor")) [RequiresRole("auditor")]"#;
    assert_eq!(sees(&log), vec![format!("g sees {line}"), format!("r sees {line}"), format!("handle sees {line}")]);
    drop(handler);
    close(app).await;
}
