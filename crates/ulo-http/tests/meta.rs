//! Declared metadata on an HTTP handler: the `#[routes]` impl's declarations apply to every
//! handler in it, the method's own come first and win per type, every declaration is listable,
//! and the handler listing a permission report reads carries each with its tier.

mod o1_support;

use std::any::type_name;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{MetaTier, ModuleDef, TypeName, injectable, routes};
use ulo_http::HttpCx;

/// A declaration with a value, as a role requirement.
#[derive(Debug)]
struct Roles(&'static str);

/// A declaration with no value, a marker.
struct Audited;

fn describe(cx: &HttpCx) -> String {
    let info = cx.exec().handler().expect("the handler's info is set on the execution");
    let winner = info.meta::<Roles>().map_or("-", |roles| roles.0);
    let all: Vec<&str> = info.meta_all::<Roles>().map(|roles| roles.0).collect();
    format!("{winner}|{}|{}", all.join(","), info.meta::<Audited>().is_some())
}

#[injectable]
struct Guarded;

#[routes]
#[meta(Roles("user"), Audited)]
impl Guarded {
    #[ulo_http::get("/inherits")]
    fn inherits(&self, cx: HttpCx) -> String {
        describe(&cx)
    }

    #[ulo_http::get("/overrides")]
    #[meta(Roles("admin"))]
    fn overrides(&self, cx: HttpCx) -> String {
        describe(&cx)
    }

    #[ulo_http::get("/twice")]
    #[meta(Roles("first"), Roles("second"))]
    fn twice(&self, cx: HttpCx) -> String {
        describe(&cx)
    }

    #[ulo_http::get("/stacked")]
    #[meta(Roles("early"))]
    #[meta(Roles("late"))]
    fn stacked(&self, cx: HttpCx) -> String {
        describe(&cx)
    }
}

#[injectable]
struct Bare;

#[routes]
impl Bare {
    #[ulo_http::get("/bare")]
    fn bare(&self, cx: HttpCx) -> String {
        describe(&cx)
    }
}

fn app(m: &mut ModuleDef<'_>) {
    m.controller::<Guarded>();
    m.controller::<Bare>();
}

async fn described(app: &o1_support::Running, path: &str) -> String {
    let reply = app.send(Call::get(path)).await;
    assert_eq!(reply.status, StatusCode::OK, "{path}");
    reply.text()
}

#[tokio::test]
async fn an_impls_declarations_reach_every_handler_in_it() {
    let app = start(app).await;

    assert_eq!(described(&app, "/inherits").await, "user|user|true");
    app.stop().await;
}

#[tokio::test]
async fn a_handlers_own_declaration_of_a_type_wins_and_the_impls_stays_listed_after_it() {
    let app = start(app).await;

    assert_eq!(described(&app, "/overrides").await, "admin|admin,user|true", "the other type is still the impl's");
    app.stop().await;
}

#[tokio::test]
async fn within_one_tier_the_first_written_declaration_answers_and_all_are_listed_in_order() {
    let app = start(app).await;

    assert_eq!(described(&app, "/twice").await, "first|first,second,user|true");
    assert_eq!(described(&app, "/stacked").await, "early|early,late,user|true", "two attributes read as one list");
    app.stop().await;
}

#[tokio::test]
async fn the_handler_listing_carries_each_declaration_with_the_tier_that_made_it() {
    let app = start(app).await;

    let handlers = app.app().handlers();
    let info = handlers.iter().find(|info| info.name() == "overrides").expect("the handler is listed");
    assert_eq!(info.route(), Some("/overrides"));
    assert_eq!(info.controller().key().type_name(), TypeName::of::<Guarded>());
    let roles: Vec<MetaTier> = info.entries().filter(|(name, _)| *name == type_name::<Roles>()).map(|(_, tier)| tier).collect();
    assert_eq!(roles, [MetaTier::Method, MetaTier::Controller], "the method's first");
    assert_eq!(info.entries().filter(|(name, _)| *name == type_name::<Audited>()).map(|(_, tier)| tier).collect::<Vec<_>>(), [MetaTier::Controller]);

    let bare = handlers.iter().find(|info| info.name() == "bare").expect("listed");
    assert_eq!(bare.entries().count(), 0);
    app.stop().await;
}
