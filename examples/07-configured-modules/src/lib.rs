use ulo::{App, AppBuilder, Dep, Keyed, Module, injectable, module, routes};

pub struct AppConfig {
    pub name: &'static str,
}

/// Global: every module sees what it exports without importing it.
#[module(global, providers = [AppConfig { name: "ledger" }], exports = [AppConfig])]
pub struct ConfigModule;

pub struct Store {
    pub url: &'static str,
}

/// A configured module: a struct with fields, identified by value. Two values are two modules;
/// the same value imported twice is one.
#[derive(Clone, PartialEq, Eq, Hash)]
#[module(providers = [Store { url: self.url }], exports = [Store])]
pub struct StoreModule {
    url: &'static str,
}

impl StoreModule {
    pub fn for_root(url: &'static str) -> Self {
        StoreModule { url }
    }
}

pub struct Primary;
pub struct Replica;

#[injectable]
pub struct LedgerController {
    config: Dep<AppConfig>,
    primary: Dep<Store, Primary>,
    replica: Dep<Store, Replica>,
}

#[routes]
impl LedgerController {
    #[ulo_http::get("/config")]
    fn config(&self) -> &'static str {
        self.config.name
    }

    #[ulo_http::get("/primary")]
    fn primary(&self) -> &'static str {
        self.primary.url
    }

    #[ulo_http::get("/replica")]
    fn replica(&self) -> &'static str {
        self.replica.url
    }
}

/// `keyed` wraps a module so its exports leave under a qualifier and its identity carries it.
pub fn primary() -> Keyed<Primary, StoreModule> {
    StoreModule::for_root("postgres://primary").keyed::<Primary>()
}

pub fn replica() -> Keyed<Replica, StoreModule> {
    StoreModule::for_root("postgres://replica").keyed::<Replica>()
}

#[module(imports = [ConfigModule, primary(), replica()], controllers = [LedgerController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
