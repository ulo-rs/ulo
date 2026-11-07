//! `Controller` is the mark of a controller, whichever way the controller is written, and a module
//! mounts its `Handlers`. A `#[module]`'s `controllers` takes a controller written by hand as it
//! takes a `#[controller]` type; a `#[controller]` type mounts handlers that its own `Handlers`
//! impl declares; a controller exported from its module is read through `Dep<C>` by a controller
//! in an importing module. `wire()` refuses a controller registered as a provider in two ways: when
//! its `Construct::dependencies` marks it, as the macros' impls do and a hand-written one does
//! with `Dependencies::mark`, an `#[injectable]` type with a hand-written `Controller` impl
//! included; and when the graph also registers the type as a controller, in the same module or
//! another, whatever binds it as a provider. It accepts a provider whose `dependencies` declares
//! what a controller's declares, and an unmarked controller type that nothing registers as a
//! controller. A generic `#[controller]` type is a controller for each type its `#[handlers]` impl
//! covers.

mod o1_support;

use http::StatusCode;
use o1_support::{Call, Root, start};
use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Handlers, Module, ModuleDef,
    ModuleIdentity, Mount, Resolver, StartupError, TypeName, WiringError, controller, handlers, injectable, module,
};
use ulo_http::{Http, HttpCx, HttpHandler, IntoReply, Method, Response};

/// A controller written without the attributes.
struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

async fn hand(cx: HttpCx) -> Result<Response, BoxError> {
    cx.exec().get::<Hand>().await?;
    Ok(String::from("hand").into_reply(&cx)?)
}

impl Controller for Hand {}

impl Handlers for Hand {
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

impl Controller for Ledger {}

impl Handlers for Ledger {
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

#[test]
fn a_hand_written_controller_marked_in_its_dependencies_is_refused_as_a_provider() {
    let (found, report) = provided(|m| {
        m.provide::<Hand>();
    });
    assert_eq!(found, vec![("Hand".to_owned(), TypeName::of::<Hand>())], "{report}");
    assert!(report.contains("help: register it with `m.controller::<Hand>()`, or list it under `controllers`"), "{report}");
}

/// A controller written by hand whose `dependencies` does not mark it: only a controller
/// registration elsewhere in the graph says what it is.
struct Unmarked;

impl Construct for Unmarked {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Unmarked)
    }
}

impl Controller for Unmarked {}

impl Handlers for Unmarked {
    fn mount(_: &mut Mount<'_>) {}
}

/// Each `ControllerProvided` entry as (key, provider module, controller module, the two lines),
/// and the report.
fn provided_beside(register: impl Module + 'static) -> (Vec<(String, String, Option<String>, u32, Option<u32>)>, String) {
    let errors = match App::builder(register).wire() {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("expected a wiring refusal, got {other}"),
        Ok(_) => panic!("wire() accepted a controller registered as a provider"),
    };
    let found = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::ControllerProvided { key, module, at, controller_at, .. } => Some((
                key.to_string(),
                module.to_string(),
                controller_at.as_ref().map(|(module, _)| module.to_string()),
                at.line(),
                controller_at.as_ref().map(|(_, at)| at.line()),
            )),
            _ => None,
        })
        .collect();
    (found, errors.to_string())
}

#[test]
fn an_unmarked_controller_provided_alone_wires_as_written() {
    let wired = App::builder(Root(|m| {
        m.provide::<Unmarked>();
    }))
    .wire();
    if let Err(error) = wired {
        panic!("nothing in this graph says `Unmarked` is a controller: {error}");
    }
}

#[test]
fn an_unmarked_controller_also_registered_as_a_controller_is_refused_naming_both_registrations() {
    let (found, report) = provided_beside(Root(|m| {
        m.controller::<Unmarked>();
        m.provide::<Unmarked>();
    }));
    let [(key, module, controller_module, at, controller_at)] = found.as_slice() else {
        panic!("one entry expected: {report}");
    };
    assert_eq!((key.as_str(), controller_module.as_deref()), ("Unmarked", Some(module.as_str())), "{report}");
    assert_eq!(*controller_at, Some(at - 1), "the controller registration's line, then the provider's: {report}");
    assert!(report.contains("registered as a provider at "), "{report}");
    assert!(report.contains("registered as a controller in "), "{report}");
    assert!(report.contains("help: remove the provider registration"), "{report}");
}

#[test]
fn an_unmarked_controller_bound_by_value_factory_or_contribution_is_refused() {
    let cases: [(&str, fn(&mut ModuleDef<'_>)); 4] = [
        ("value", |m| {
            m.controller::<Unmarked>();
            m.value(Unmarked);
        }),
        ("with", |m| {
            m.controller::<Unmarked>();
            m.with(|| async { Unmarked });
        }),
        ("singleton", |m| {
            m.controller::<Unmarked>();
            m.singleton(|| async { Unmarked });
        }),
        ("contribute value", |m| {
            m.controller::<Unmarked>();
            m.contribute::<Unmarked>().value(std::sync::Arc::new(Unmarked));
        }),
    ];
    for (form, register) in cases {
        let (found, report) = provided_beside(Root(register));
        assert_eq!(found.len(), 1, "{form}: {report}");
        assert!(found[0].2.is_some(), "{form}: the entry names the controller registration: {report}");
    }
}

/// Registers `Unmarked` as a controller.
struct ControllerSide;

impl Module for ControllerSide {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Unmarked>();
    }
}

/// Binds `Unmarked` by value.
struct ProviderSide;

impl Module for ProviderSide {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Unmarked);
    }
}

#[test]
fn a_controller_in_one_module_bound_as_a_provider_in_another_is_refused_naming_both_modules() {
    let (found, report) = provided_beside(Root(|m| {
        m.import(ControllerSide);
        m.import(ProviderSide);
    }));
    let [(_, module, controller_module, _, _)] = found.as_slice() else {
        panic!("one entry expected: {report}");
    };
    assert_eq!((module.as_str(), controller_module.as_deref()), ("ProviderSide", Some("ControllerSide")), "{report}");
}

/// Registers `Checkout`, which the `Construct` impl `#[controller]` writes marks, as a controller.
struct MarkedControllerSide;

impl Module for MarkedControllerSide {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Checkout>();
    }
}

/// Registers `Checkout` with `provide`.
struct MarkedProviderSide;

impl Module for MarkedProviderSide {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<Checkout>();
    }
}

#[test]
fn a_marked_controller_also_registered_as_a_controller_is_reported_once_naming_both_registrations() {
    let (found, report) = provided_beside(Root(|m| {
        m.import(MarkedControllerSide);
        m.import(MarkedProviderSide);
    }));
    let [(_, module, controller_module, _, _)] = found.as_slice() else {
        panic!("one entry for the one provider registration, which both checks find: {report}");
    };
    assert_eq!((module.as_str(), controller_module.as_deref()), ("MarkedProviderSide", Some("MarkedControllerSide")), "{report}");
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
