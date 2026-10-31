//! A controller written without the attributes, through `HttpHandler` and `HandlerSpec`: it answers
//! what its attribute-written twin answers, with the same `Routing`, `HandlerInfo` and metadata;
//! its `Path<T>` declaration is checked against its route as a parameter is; its route may be built
//! at runtime; and a handler mounted with no route is refused naming its controller.

mod o1_support;

use std::time::Duration;

use http::StatusCode;
use o1_support::{Call, refusal_names, refusals, refused, start};
use ulo::scope::Auto;
use ulo::{
    BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Metadata, ModuleDef, Mount, Resolver, Shape,
    TypeName, injectable, routes,
};
use ulo_http::{FromCall, Http, HttpCx, HttpHandler, IntoReply, Method, Path, Response, Timeout};

#[injectable]
struct Macro;

#[routes]
impl Macro {
    #[ulo_http::get("/m/users/{id}")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    fn user(&self, cx: HttpCx, Path(id): Path<u64>) -> String {
        format!("{id}|{}", cx.route().unwrap_or("-"))
    }
}

struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

impl Hand {
    fn user(&self, cx: &HttpCx, id: u64) -> String {
        format!("{id}|{}", cx.route().unwrap_or("-"))
    }
}

async fn user(cx: HttpCx) -> Result<Response, BoxError> {
    let Path(id) = <Path<u64> as FromCall<Http>>::from_call(&cx).await?;
    let hand = cx.exec().get::<Hand>().await?;
    Ok(hand.user(&cx, id).into_reply(&cx)?)
}

impl Controller for Hand {
    fn mount(m: &mut Mount<'_>) {
        let mut meta = Metadata::new();
        meta.method(Timeout::after(Duration::from_secs(5)));
        let handler = HttpHandler::new(Method::GET, user).reads_path::<u64>();
        m.handler(HandlerSpec::<Http, _>::new("user", handler).route("/h/users/{id}").meta(meta));
    }
}

fn both(m: &mut ModuleDef<'_>) {
    m.controller::<Macro>();
    m.controller::<Hand>();
}

#[tokio::test]
async fn a_hand_written_handler_answers_as_its_attribute_written_twin() {
    let running = start(both).await;
    for (prefix, controller) in [("m", TypeName::of::<Macro>()), ("h", TypeName::of::<Hand>())] {
        let reply = running.send(Call::get(&format!("/{prefix}/users/7"))).await;
        assert_eq!(reply.status, StatusCode::OK, "{prefix}");
        assert_eq!(reply.header("content-type"), Some("text/plain; charset=utf-8"), "{prefix}");
        assert_eq!(reply.text(), format!("7|/{prefix}/users/{{id}}"), "{prefix}");
        assert_eq!(reply.matched(), Some((format!("/{prefix}/users/{{id}}"), "user")), "{prefix}");

        let malformed = running.send(Call::get(&format!("/{prefix}/users/seven"))).await;
        assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{prefix}");

        let handlers = running.app().handlers();
        let info = handlers.iter().find(|info| info.controller().key().type_name() == controller).expect("the handler");
        assert_eq!(info.transport(), "http", "{prefix}");
        assert_eq!(info.name(), "user", "{prefix}");
        assert_eq!(info.route(), Some(format!("/{prefix}/users/{{id}}").as_str()), "{prefix}");
        assert_eq!(info.prefix(), None, "{prefix}");
        assert_eq!(info.shape(), Shape::Unary, "{prefix}");
        assert_eq!(info.meta::<Timeout>(), Some(&Timeout::after(Duration::from_secs(5))), "{prefix}");
    }
    running.stop().await;
}

struct Misfit;

impl Construct for Misfit {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Misfit)
    }
}

impl Controller for Misfit {
    fn mount(m: &mut Mount<'_>) {
        let handler = HttpHandler::new(Method::GET, |_cx: HttpCx| async { Ok::<_, BoxError>(Response::default()) })
            .reads_path::<(u64, u64)>();
        m.handler(HandlerSpec::<Http, _>::new("pair", handler).route("/one/{id}"));
    }
}

#[tokio::test]
async fn a_declared_path_type_that_does_not_fit_the_route_is_refused_naming_the_controller_and_the_type() {
    let error = refused(|m| {
        m.controller::<Misfit>();
    })
    .await;

    assert_eq!(refusals(&error), 1, "{error}");
    let names = refusal_names(&error);
    assert!(names.contains(&TypeName::of::<Misfit>()), "{names:?}");
    assert!(names.contains(&TypeName::of::<(u64, u64)>()), "{names:?}");
    assert!(error.to_string().contains("`Path<(u64, u64)>` does not fit the route `/one/{id}`"), "{error}");
}

/// The route as configuration would give it, built when the controller mounts.
struct Configured;

impl Construct for Configured {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Configured)
    }
}

impl Controller for Configured {
    fn mount(m: &mut Mount<'_>) {
        let route = ["status", "v2"].join("/");
        let handler = HttpHandler::new(Method::GET, |cx: HttpCx| async move { Ok("up".into_reply(&cx)?) });
        m.handler(HandlerSpec::<Http, _>::new("status", handler).route(format!("/{route}")));
    }
}

#[tokio::test]
async fn a_route_built_at_mount_is_the_route_the_router_serves() {
    let running = start(|m| {
        m.controller::<Configured>();
    })
    .await;
    let reply = running.send(Call::get("/status/v2")).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "up");
    assert_eq!(reply.matched(), Some((String::from("/status/v2"), "status")));
    running.stop().await;
}

struct Routeless;

impl Construct for Routeless {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Routeless)
    }
}

impl Controller for Routeless {
    fn mount(m: &mut Mount<'_>) {
        let handler = HttpHandler::new(Method::GET, |_cx: HttpCx| async { Ok::<_, BoxError>(Response::default()) });
        m.handler(HandlerSpec::<Http, _>::new("nowhere", handler));
    }
}

#[tokio::test]
async fn a_handler_mounted_with_no_route_is_refused_naming_its_controller() {
    let error = refused(|m| {
        m.controller::<Routeless>();
    })
    .await;

    assert_eq!(refusals(&error), 1, "{error}");
    assert_eq!(refusal_names(&error), [TypeName::of::<Routeless>()]);
    assert!(error.to_string().contains("::nowhere` is mounted for HTTP with no route; `HandlerSpec::route` sets it"), "{error}");
}
