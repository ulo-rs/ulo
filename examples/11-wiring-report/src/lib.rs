use ulo::{App, AppBuilder, Dep, Module, ModuleDef, ModuleIdentity, injectable, module};

pub trait Mailer: Send + Sync {}
pub trait Repo: Send + Sync {}

/// Reads a binding nothing provides.
#[injectable]
pub struct Notifier {
    _mailer: Dep<dyn Mailer>,
}

/// Two modules export `dyn Repo`, and the root reads it.
#[injectable]
pub struct SqlRepo;
#[injectable]
pub struct MemoryRepo;
impl Repo for SqlRepo {}
impl Repo for MemoryRepo {}

#[module(providers = [SqlRepo as dyn Repo], exports = [dyn Repo])]
pub struct SqlModule;

#[module(providers = [MemoryRepo as dyn Repo], exports = [dyn Repo])]
pub struct MemoryModule;

#[injectable]
pub struct Reports {
    _repo: Dep<dyn Repo>,
}

/// A singleton cannot depend on something built per request.
#[injectable(execution)]
pub struct Session;

#[injectable(singleton)]
pub struct Directory {
    _session: Dep<Session>,
}

/// Two providers that need each other.
#[injectable]
pub struct Chicken {
    _egg: Dep<Egg>,
}

#[injectable]
pub struct Egg {
    _chicken: Dep<Chicken>,
}

/// Written against `ModuleDef`, so a duplicate can be registered twice.
pub struct TwiceModule;

impl Module for TwiceModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(7_u32);
        m.value(8_u32);
    }
}

#[module(
    imports = [SqlModule, MemoryModule, TwiceModule],
    providers = [Notifier, Reports, Session, Directory, Chicken, Egg],
)]
pub struct BrokenModule;

pub fn builder() -> AppBuilder {
    App::builder(BrokenModule)
}
