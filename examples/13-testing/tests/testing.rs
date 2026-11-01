use std::sync::Arc;
use std::time::Duration;

use ex_13_testing::{AppModule, BillingModule, Ledger, MailModule, Mailer, RefundsModule, Stamp, Uptime};
use ex_support::{get, listen};
use ulo::{Dep, Signal, StartupError, TestApp, WiringError, injectable, module};
use ulo_runtime_tokio::Tokio;

#[injectable]
struct FakeMailer;

impl Mailer for FakeMailer {
    fn send(&self, to: &str) -> String {
        format!("fake to {to}")
    }
}

fn test_app() -> TestApp {
    TestApp::of(AppModule).runtime(Tokio::current())
}

#[tokio::test]
async fn overrides_by_value_and_factory_scoped_to_one_module_answer_in_place_of_the_real_ones() {
    let app = test_app()
        .override_value::<dyn Mailer>(Arc::new(FakeMailer))
        .override_factory(|| async { Stamp("2026-01-01".to_owned()) })
        .override_value::<Ledger>(Arc::new(Ledger("test")))
        .in_module::<BillingModule>()
        .connect()
        .await
        .unwrap();
    let refunds = app.module::<RefundsModule>().unwrap().get::<Ledger>().await.unwrap();
    assert_eq!(refunds.0, "refunds", "the scoped override leaves RefundsModule's Ledger alone");
    let server = listen(app).await;
    assert_eq!(get(server.addr, "/signup").await.body, "fake to ada at 2026-01-01 in test");
    server.stop().await.unwrap();
}

#[module(providers = [FakeMailer as dyn Mailer], exports = [dyn Mailer])]
struct FakeMailModule;

#[tokio::test]
async fn a_replaced_module_stands_in_for_the_original() {
    let app = test_app().replace_module(MailModule, FakeMailModule).connect().await.unwrap();
    let server = listen(app).await;
    assert_eq!(get(server.addr, "/signup").await.body, "fake to ada at production in billing");
    server.stop().await.unwrap();
}

/// `Uptime` reads the app's clock, which is tokio's under `Tokio::current()`: a paused test clock
/// sets what the app reads as time. It is read in-process because a running server holds timers
/// of its own, and the paused clock jumps to them while a request is in flight.
#[tokio::test(start_paused = true)]
async fn a_paused_clock_sets_what_the_app_reads_as_time() {
    let app = test_app().connect().await.unwrap();
    let uptime: Dep<Uptime> = app.get().await.unwrap();
    assert_eq!(uptime.elapsed(), Duration::ZERO);
    tokio::time::advance(Duration::from_secs(90)).await;
    assert_eq!(uptime.elapsed(), Duration::from_secs(90));
    app.close(Signal::new("test")).await.unwrap();
}

struct Unbound;

#[tokio::test]
async fn an_override_of_a_key_nothing_binds_fails_wiring_as_matching_no_binding() {
    let Err(StartupError::Wiring(errors)) = test_app().override_value::<Unbound>(Arc::new(Unbound)).wire() else {
        panic!("a stale override should fail wiring");
    };
    let [WiringError::OverrideUnmatched { key, keyed, at }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one unmatched override, got {errors}");
    };
    assert_eq!(key.to_string(), "Unbound");
    assert!(keyed.is_none());
    assert!(at.file().ends_with("13-testing/tests/testing.rs"), "{at}");
    assert!(errors.to_string().contains("override of `Unbound` matches no binding"), "{errors}");
}

#[tokio::test]
async fn an_unscoped_override_of_a_key_two_modules_bind_lists_both_modules() {
    let Err(StartupError::Wiring(errors)) = test_app().override_value::<Ledger>(Arc::new(Ledger("test"))).wire() else {
        panic!("an ambiguous override should fail wiring");
    };
    let [WiringError::OverrideAmbiguous { key, matches, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one ambiguous override, got {errors}");
    };
    assert_eq!(key.to_string(), "Ledger");
    let modules: Vec<String> = matches.iter().map(|m| m.to_string()).collect();
    assert_eq!(modules, ["BillingModule", "RefundsModule"]);
}
