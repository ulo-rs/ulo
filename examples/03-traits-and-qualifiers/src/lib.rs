use ulo::{App, AppBuilder, Dep, Module, ModuleDef, ModuleIdentity, injectable, module, routes};

/// A trait a provider is bound under. `Send + Sync` is what `Dep<dyn UserRepo>` requires.
pub trait UserRepo: Send + Sync {
    fn name(&self) -> &'static str;
}

#[injectable]
pub struct PgUserRepo;

impl UserRepo for PgUserRepo {
    fn name(&self) -> &'static str {
        "pg"
    }
}

/// Two instances of one type, told apart by a qualifier: any `'static` type will do.
pub struct Primary;
pub struct Replica;

pub struct Db(pub &'static str);

/// `#[module]` has no spelling for a qualifier, so this module is written against `ModuleDef`.
pub struct DbModule;

impl Module for DbModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.singleton(|| async { Db("primary") }).qualified::<Primary>();
        m.singleton(|| async { Db("replica") }).qualified::<Replica>();
        m.export_qualified::<Db, Primary>();
        m.export_qualified::<Db, Replica>();
    }
}

#[injectable]
pub struct UserController {
    repo: Dep<dyn UserRepo>,
    primary: Dep<Db, Primary>,
    replica: Dep<Db, Replica>,
}

#[routes]
impl UserController {
    #[ulo_http::get("/repo")]
    fn repo(&self) -> &'static str {
        self.repo.name()
    }

    #[ulo_http::get("/write")]
    fn write(&self) -> &'static str {
        self.primary.0
    }

    #[ulo_http::get("/read")]
    fn read(&self) -> &'static str {
        self.replica.0
    }
}

#[module(
    imports = [DbModule],
    providers = [PgUserRepo as dyn UserRepo],
    controllers = [UserController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
