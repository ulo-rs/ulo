//! Example 06 without a macro: `Construct` by hand for the providers and the controller,
//! `Controller::mount` for its route, `Module::register` for the modules.

use ulo::scope::Auto;
use ulo::{
    App, AppBuilder, BoxError, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Module, ModuleDef,
    ModuleIdentity, Mount, Resolver,
};
use ulo_http::{Http, HttpCx, HttpHandler, IntoReply, Method, Response};

pub struct PasswordHasher;

impl PasswordHasher {
    pub fn hash(&self, password: &str) -> String {
        format!("hashed:{password}")
    }
}

impl Construct for PasswordHasher {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(PasswordHasher)
    }
}

pub struct UserService {
    hasher: Dep<PasswordHasher>,
}

impl UserService {
    pub fn register(&self, name: &str) -> String {
        format!("{name} registered with {}", self.hasher.hash("secret"))
    }
}

impl Construct for UserService {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<PasswordHasher>>("hasher");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(UserService { hasher: r.dep::<PasswordHasher>().await? })
    }
}

pub struct InvoiceService {
    users: Dep<UserService>,
}

impl InvoiceService {
    pub fn open_for(&self, name: &str) -> String {
        format!("invoice for {}", self.users.register(name))
    }
}

impl Construct for InvoiceService {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<UserService>>("users");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(InvoiceService { users: r.dep::<UserService>().await? })
    }
}

pub struct InvoiceController {
    invoices: Dep<InvoiceService>,
}

impl InvoiceController {
    fn open(&self) -> String {
        self.invoices.open_for("ada")
    }
}

impl Construct for InvoiceController {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<InvoiceService>>("invoices");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(InvoiceController { invoices: r.dep::<InvoiceService>().await? })
    }
}

/// What `#[ulo_http::get("/")]` on `open` writes: one GET handler at `/`, named after the method.
impl Controller for InvoiceController {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Http, _>::new("open", HttpHandler::new(Method::GET, open)).route("/"));
    }
}

async fn open(cx: HttpCx) -> Result<Response, BoxError> {
    let controller = cx.exec().get::<InvoiceController>().await?;
    Ok(controller.open().into_reply(&cx)?)
}

/// Reads a binding `UsersModule` does not export.
pub struct Snooper {
    _hasher: Dep<PasswordHasher>,
}

impl Construct for Snooper {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<PasswordHasher>>("_hasher");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Snooper { _hasher: r.dep::<PasswordHasher>().await? })
    }
}

pub struct UsersModule;

impl Module for UsersModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<PasswordHasher>();
        m.provide::<UserService>();
        m.export::<UserService>();
    }
}

pub struct BillingModule;

impl Module for BillingModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(UsersModule);
        m.provide::<InvoiceService>();
        m.controller::<InvoiceController>();
    }
}

pub struct AppModule;

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(BillingModule);
    }
}

pub struct SnoopingModule;

impl Module for SnoopingModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(UsersModule);
        m.provide::<Snooper>();
    }
}

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
