//! The handler listing names one HTTP operation per handler, method, prefix and route, with the
//! module that registers it, and is read from the wired graph before any singleton is built.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{App, HandlerInfo, Module, ModuleDef, ModuleIdentity, controller, handlers};

#[controller]
struct Users;

#[handlers]
impl Users {
    #[ulo_http::get("/{id}")]
    fn show(&self) -> &'static str {
        "user"
    }

    #[ulo_http::post("/")]
    fn create(&self) -> &'static str {
        "created"
    }
}

#[controller]
struct Health;

#[handlers]
impl Health {
    #[ulo_http::delete("/cache")]
    fn flush(&self) -> &'static str {
        "flushed"
    }
}

struct Database;

/// Registers `Users` under `/users` beside a singleton that fails to build, counting each attempt.
#[derive(Clone)]
struct UsersModule(Arc<AtomicUsize>);

impl Module for UsersModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Users>().at("/users");
        let attempts = Arc::clone(&self.0);
        m.try_singleton(move || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err::<Database, _>(std::io::Error::other("the database is unreachable")) }
        });
    }
}

#[derive(Clone)]
struct Root(UsersModule);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(self.0.clone());
        m.controller::<Health>();
    }
}

fn operations(handlers: &[Arc<HandlerInfo>]) -> Vec<(String, String, String, &'static str)> {
    handlers
        .iter()
        .map(|info| {
            let path = format!("{}{}", info.prefix().unwrap_or(""), info.route().unwrap_or(""));
            let method = info.method().expect("an HTTP handler has a method").to_owned();
            (method, path, info.module_name().to_string(), info.name())
        })
        .collect()
}

#[tokio::test]
async fn a_wired_app_lists_each_operation_with_its_module_and_builds_nothing() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let wired = App::builder(Root(UsersModule(Arc::clone(&attempts)))).wire().expect("wires");

    let handlers = wired.handlers();
    let mut listed = operations(&handlers);
    listed.sort();
    assert_eq!(
        listed,
        [
            ("DELETE".to_owned(), "/cache".to_owned(), "Root".to_owned(), "flush"),
            ("GET".to_owned(), "/users/{id}".to_owned(), "UsersModule".to_owned(), "show"),
            ("POST".to_owned(), "/users/".to_owned(), "UsersModule".to_owned(), "create"),
        ]
    );
    let users: Vec<&str> =
        handlers.iter().filter(|info| info.module() == &ModuleIdentity::of_type::<UsersModule>()).map(|info| info.name()).collect();
    assert_eq!(users, ["show", "create"], "a module filter keeps the module's own handlers, in mount order");
    assert_eq!(attempts.load(Ordering::SeqCst), 0, "listing the handlers built no singleton");

    assert!(wired.connect().await.is_err(), "the singleton the listing did not build fails a connect");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_connected_app_lists_the_same_handlers() {
    #[derive(Clone)]
    struct Plain;
    impl Module for Plain {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, m: &mut ModuleDef<'_>) {
            m.controller::<Users>().at("/users");
            m.controller::<Health>();
        }
    }

    let wired = App::builder(Plain).wire().expect("wires");
    let before = operations(&wired.handlers());
    let connected = wired.connect().await.expect("connects");
    assert_eq!(operations(&connected.handle().handlers()), before);
    connected.close(ulo::Signal::new("test")).await.expect("closes");
}
