//! `Controller` is the mark of a controller, whichever way the controller is written. A
//! `#[module]`'s `controllers` takes a controller written by hand as it takes a `#[controller]`
//! type; a `#[controller]` type mounts handlers that its own `Handlers` impl declares; a
//! controller exported from its module is read through `Dep<C>` by a controller in an importing
//! module; and `wire()` refuses a controller registered as a provider when its `Construct` impl is
//! one the macros wrote, an `#[injectable]` type with a hand-written `Controller` impl included,
//! and accepts a provider whose `dependencies` declares what a controller's declares. A generic
//! `#[controller]` type is a controller for each type its `#[handlers]` impl covers.

mod o1_support;

use http::StatusCode;
use o1_support::{Call, Root, start};
use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Handlers, ModuleDef, Mount, Resolver,
    StartupError, TypeName, WiringError, controller, handlers, injectable, module,
};
use ulo_http::{Http, HttpCx, HttpHandler, IntoReply, Method, Response};

/// A controller written without the attributes.
struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

async fn hand(cx: HttpCx) -> Result<Response, BoxError> {
    cx.exec().get::<Hand>().await?;
    Ok(String::from("hand").into_reply(&cx)?)
}

impl Controller for Hand {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Http, _>::new("get", HttpHandler::new(Method::GET, hand)).route("/hand"));
    }
}

#[module(controllers = [Hand])]
struct HandModule;

/// A `#[controller]` type whose handler is declared by hand.
#[controller]
struct HandMounted;

async fn hand_mounted(cx: HttpCx) -> Result<Response, BoxError> {
    cx.exec().get::<HandMounted>().await?;
    Ok(String::from("hand-mounted").into_reply(&cx)?)
}

impl Handlers for HandMounted {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Http, _>::new("get", HttpHandler::new(Method::GET, hand_mounted)).route("/hand-mounted"));
    }
}

#[module(controllers = [HandMounted])]
struct HandMountedModule;

#[tokio::test]
async fn a_hand_written_controller_is_listed_under_controllers() {
    let app = start(|m| {
        m.import(HandModule);
    })
    .await;
    let reply = app.send(Call::get("/hand")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "hand".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn a_controller_mounts_the_handlers_its_handlers_impl_declares_by_hand() {
    let app = start(|m| {
        m.import(HandMountedModule);
    })
    .await;
    let reply = app.send(Call::get("/hand-mounted")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "hand-mounted".to_owned()));
    app.stop().await;
}

#[controller]
struct Catalog;

#[handlers]
impl Catalog {
    #[ulo_http::get("/catalog")]
    fn get(&self) -> String {
        self.title().to_owned()
    }
}

impl Catalog {
    fn title(&self) -> &'static str {
        "the catalog"
    }
}

#[module(controllers = [Catalog], exports = [Catalog])]
struct CatalogModule;

#[controller]
struct Storefront {
    catalog: Dep<Catalog>,
}

#[handlers]
impl Storefront {
    #[ulo_http::get("/storefront")]
    fn get(&self) -> String {
        format!("storefront over {}", self.catalog.title())
    }
}

#[module(imports = [CatalogModule], controllers = [Storefront])]
struct StorefrontModule;

#[tokio::test]
async fn an_exported_controller_is_read_by_a_controller_in_an_importing_module() {
    let app = start(|m| {
        m.import(StorefrontModule);
    })
    .await;
    let reply = app.send(Call::get("/storefront")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "storefront over the catalog".to_owned()));
    let reply = app.send(Call::get("/catalog")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "the catalog".to_owned()), "the exported controller serves too");
    app.stop().await;
}

#[controller]
struct Checkout;

#[handlers]
impl Checkout {
    #[ulo_http::get("/checkout")]
    fn get(&self) -> String {
        "checkout".to_owned()
    }
}

/// A controller all the same: the probe in the `Construct` impl `#[injectable]` writes reads the
/// `Controller` impl beside it.
#[injectable]
struct Ledger;

impl Controller for Ledger {
    fn mount(_: &mut Mount<'_>) {}
}

#[injectable]
struct Plain;

/// The `ControllerProvided` entries `wire()` refuses `register` with, as (key, controller), and
/// the report.
fn provided(register: fn(&mut ModuleDef<'_>)) -> (Vec<(String, TypeName)>, String) {
    let errors = match App::builder(Root(register)).wire() {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("expected a wiring refusal, got {other}"),
        Ok(_) => panic!("wire() accepted a controller registered as a provider"),
    };
    let found = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::ControllerProvided { key, controller, .. } => Some((key.to_string(), *controller)),
            _ => None,
        })
        .collect();
    (found, errors.to_string())
}

#[test]
fn a_controller_registered_with_provide_is_refused() {
    let (found, report) = provided(|m| {
        m.provide::<Checkout>();
    });
    assert_eq!(found, vec![("Checkout".to_owned(), TypeName::of::<Checkout>())], "{report}");
    assert!(report.contains("help: register it with `m.controller::<Checkout>()`, or list it under `controllers`"), "{report}");
}

#[test]
fn a_controller_built_by_a_factory_or_contributed_is_refused() {
    let (found, report) = provided(|m| {
        m.provide_with(|| async { Checkout });
    });
    assert_eq!(found, vec![("Checkout".to_owned(), TypeName::of::<Checkout>())], "provide_with: {report}");
    let (found, report) = provided(|m| {
        m.try_provide_with(|| async { Ok::<_, BoxError>(Checkout) });
    });
    assert_eq!(found, vec![("Checkout".to_owned(), TypeName::of::<Checkout>())], "try_provide_with: {report}");
    let (found, report) = provided(|m| {
        m.contribute::<Checkout>().provide::<Checkout>(|a| a);
    });
    assert_eq!(found, vec![("Checkout (collection)".to_owned(), TypeName::of::<Checkout>())], "contribute: {report}");
}

#[test]
fn an_injectable_with_a_hand_written_controller_impl_is_refused_as_a_provider() {
    let (found, report) = provided(|m| {
        m.provide::<Ledger>();
        m.provide::<Plain>();
    });
    assert_eq!(found, vec![("Ledger".to_owned(), TypeName::of::<Ledger>())], "the controller alone: {report}");
}

#[test]
fn a_controller_under_controllers_and_a_provider_under_provide_wire() {
    let wired = App::builder(Root(|m| {
        m.controller::<Checkout>();
        m.controller::<Ledger>();
        m.provide::<Plain>();
    }))
    .wire();
    if let Err(error) = wired {
        panic!("wire() refused controllers registered as controllers: {error}");
    }
}

/// A provider whose hand-written `dependencies` declares what `Checkout`'s declares.
struct Beside;

impl Construct for Beside {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        <Checkout as Construct>::dependencies(d);
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Beside)
    }
}

#[test]
fn a_provider_reusing_a_controller_s_dependencies_is_not_refused() {
    let wired = App::builder(Root(|m| {
        m.provide::<Beside>();
    }))
    .wire();
    if let Err(error) = wired {
        panic!("wire() refused a provider that is not a controller: {error}");
    }
}

pub trait Titled: Send + Sync + 'static {
    const TITLE: &'static str;
}

pub struct Books;

impl Titled for Books {
    const TITLE: &'static str = "books";
}

/// Generic over any `T`, with handlers only for a `T: Titled`.
#[controller]
struct Shelf<T: Send + Sync + 'static> {
    #[controller(default)]
    _of: std::marker::PhantomData<fn() -> T>,
}

#[handlers]
impl<T: Titled> Shelf<T> {
    #[ulo_http::get("/shelf")]
    fn get(&self) -> &'static str {
        T::TITLE
    }
}

#[module(controllers = [Shelf<Books>])]
struct ShelfModule;

#[tokio::test]
async fn a_generic_controller_is_a_controller_where_its_handlers_impl_applies() {
    let app = start(|m| {
        m.import(ShelfModule);
    })
    .await;
    let reply = app.send(Call::get("/shelf")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "books".to_owned()));
    app.stop().await;
}
