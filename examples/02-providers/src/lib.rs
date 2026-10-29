use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use ulo::{App, AppBuilder, Dep, injectable, module, routes};

/// A value the module binds; `Greeter::new` reads it like any provider.
pub struct Settings {
    pub salutation: &'static str,
}

/// Struct style: every field is injected unless marked `default`, which owns its state.
#[injectable]
pub struct Counter {
    #[injectable(default)]
    hits: AtomicU64,
}

impl Counter {
    pub fn next(&self) -> u64 {
        self.hits.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[derive(Debug)]
pub struct EmptySalutation;

impl fmt::Display for EmptySalutation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the salutation is empty")
    }
}

impl std::error::Error for EmptySalutation {}

pub struct Greeter {
    counter: Dep<Counter>,
    salutation: &'static str,
}

/// Constructor style: the parameters are injected, and the body may await and fail.
#[injectable]
impl Greeter {
    pub async fn new(counter: Dep<Counter>, settings: Dep<Settings>) -> Result<Self, EmptySalutation> {
        if settings.salutation.is_empty() {
            return Err(EmptySalutation);
        }
        Ok(Greeter { counter, salutation: settings.salutation })
    }

    pub fn greet(&self) -> String {
        format!("{} #{}", self.salutation, self.counter.next())
    }
}

#[injectable]
pub struct HelloController {
    greeter: Dep<Greeter>,
}

#[routes]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> String {
        self.greeter.greet()
    }
}

#[module(
    providers = [Counter, Greeter, Settings { salutation: "Hello" }],
    controllers = [HelloController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
