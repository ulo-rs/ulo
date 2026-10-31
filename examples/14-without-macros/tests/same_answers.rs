//! One test body, run against example 06's macro-written modules and controller and this
//! example's hand-written ones. A divergence between the two spellings shows up as the same
//! assertion failing for one.

use std::future::Future;

use ulo::{App, AppBuilder, Connected, Shape, StartupError, WiringError, injectable, module};
use ulo_runtime_tokio::Tokio;

const INVOICE: &str = "invoice for ada registered with hashed:secret";

async fn connected(builder: AppBuilder) -> App<Connected> {
    builder.runtime(Tokio::current()).wire().unwrap().connect().await.unwrap()
}

/// What billing answers, read through its module handle, and the names of the modules involved.
async fn the_app_answers<F: Future<Output = String>>(builder: AppBuilder, billing_answer: impl FnOnce(App<Connected>) -> F) {
    assert_eq!(billing_answer(connected(builder).await).await, INVOICE);
}

#[tokio::test]
async fn the_macro_written_app_answers() {
    the_app_answers(ex_06_modules::builder(), async |app| {
        let billing = app.module::<ex_06_modules::BillingModule>().unwrap();
        assert_eq!(billing.name().to_string(), "BillingModule");
        billing.get::<ex_06_modules::InvoiceService>().await.unwrap().open_for("ada")
    })
    .await;
}

#[tokio::test]
async fn the_hand_written_app_gives_the_same_answer() {
    the_app_answers(ex_14_without_macros::builder(), async |app| {
        let billing = app.module::<ex_14_without_macros::BillingModule>().unwrap();
        assert_eq!(billing.name().to_string(), "BillingModule");
        billing.get::<ex_14_without_macros::InvoiceService>().await.unwrap().open_for("ada")
    })
    .await;
}

#[injectable]
struct Snooper {
    _hasher: ulo::Dep<ex_06_modules::PasswordHasher>,
}

#[module(imports = [ex_06_modules::UsersModule], providers = [Snooper])]
struct MacroSnoopingModule;

/// Both spellings refuse a read of the unexported binding, naming the same key and module.
fn refusal(builder: AppBuilder, key: &str, consumer: &str, module: &str) {
    let Err(StartupError::Wiring(errors)) = builder.wire() else { panic!("wiring should fail") };
    let [WiringError::Missing { key: missing, consumer: reader, module: reading, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one missing dependency, got {errors}");
    };
    assert_eq!(missing.to_string(), key);
    assert_eq!(reader, consumer);
    assert_eq!(reading.to_string(), module);
}

#[test]
fn both_spellings_refuse_a_read_of_a_private_binding_the_same_way() {
    refusal(App::builder(MacroSnoopingModule), "PasswordHasher", "Snooper (field `_hasher`)", "MacroSnoopingModule");
    refusal(App::builder(ex_14_without_macros::SnoopingModule), "PasswordHasher", "Snooper (field `_hasher`)", "SnoopingModule");
}

/// What `GET /` answers, and the handler the app reports for it, field by field.
async fn the_route_answers(builder: AppBuilder, controller: &str) {
    let served = ex_support::start(builder).await;
    let reply = ex_support::get(served.addr, "/").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(reply.body, INVOICE);

    let handlers = served.handle.handlers();
    let [info] = &handlers[..] else { panic!("expected one handler, got {}", handlers.len()) };
    assert_eq!(info.transport(), "http");
    assert_eq!(info.controller().to_string(), controller);
    assert_eq!(info.name(), "open");
    assert_eq!(info.route(), Some("/"));
    assert_eq!(info.prefix(), None);
    assert_eq!(info.shape(), Shape::Unary);
    assert_eq!(info.entries().count(), 0);
    served.stop().await.unwrap();
}

#[tokio::test]
async fn the_macro_written_route_answers() {
    the_route_answers(ex_06_modules::builder(), "InvoiceController").await;
}

#[tokio::test]
async fn the_hand_written_route_gives_the_same_answer() {
    the_route_answers(ex_14_without_macros::builder(), "InvoiceController").await;
}
