//! `#[routes]` over every HTTP verb attribute, with the extractors, the reply forms, the enhancer
//! spellings at both tiers, `#[meta]`, a generic controller, one declared inside a fn body, two
//! written by a `macro_rules!` macro and `self: Arc<Self>`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use serde::Deserialize;
use ulo::{Dep, Runtime, Timer, injectable, routes};
use ulo_http::{Event, HttpCx, Json, Path, Query, Sse, Timeout};
use ulo_transport::Valid;

use crate::derives::{Item, Lookup, Refusal};
use crate::di::Store;
use crate::enhancers::{Allow, Pass, Relay, Tag};

#[derive(Debug, Deserialize)]
pub struct Page {
    pub page: u32,
}

#[injectable]
pub struct Pages {
    pub store: Dep<Store>,
}

#[routes]
#[guards(Allow, http = Allow, value = Allow, with = || Allow)]
#[interceptors(Pass, value = Pass)]
#[error_handlers(Relay, value = Relay)]
#[meta(Tag("pages"), Timeout::after(std::time::Duration::from_secs(5)))]
impl Pages {
    #[ulo_http::get("/")]
    fn index(&self) -> &'static str {
        "index"
    }

    #[ulo_http::get("/items/{id}")]
    async fn item(&self, id: Path<u32>, page: Query<Page>) -> Result<String, Lookup> {
        if id.0 == 0 { Err(Lookup::Missing) } else { Ok(format!("{}:{}", id.0, page.0.page)) }
    }

    #[ulo_http::post("/items")]
    #[guards(value = Allow)]
    #[interceptors(with = || Pass)]
    #[error_handlers(Relay)]
    #[meta(Tag("create"))]
    async fn create(&self, item: Valid<Json<Item>>) -> Result<Json<String>, Refusal> {
        Ok(Json(item.into_inner().0.name))
    }

    #[ulo_http::put("/items/{id}")]
    fn replace(&self, id: Path<u32>, cx: HttpCx) -> String {
        format!("{} {}", id.0, cx.exec().is_draining())
    }

    #[ulo_http::patch("/items/{id}")]
    fn amend(&self, id: Path<u32>) -> String {
        id.0.to_string()
    }

    #[ulo_http::delete("/items/{id}")]
    async fn remove(self: Arc<Self>, id: Path<u32>) -> Result<(), Refusal> {
        let _ = (self.store.hits, id.0);
        Ok(())
    }

    #[ulo_http::head("/items")]
    fn probe(&self) {}

    #[ulo_http::options("/items")]
    fn allowed(&self) -> &'static str {
        "GET, POST"
    }

    #[ulo_http::get("/clock")]
    async fn clock(&self, timer: Dep<dyn Timer>, runtime: Dep<dyn Runtime>) -> &'static str {
        timer.sleep(Duration::ZERO).await;
        let _ = runtime;
        "tick"
    }

    #[ulo_http::post("/borrowed")]
    fn borrowed(&self, body: Json<Cow<'static, str>>, query: Query<HashMap<Cow<'static, str>, String>>) -> String {
        format!("{} {}", body.0, query.0.len())
    }

    #[ulo_http::get("/events")]
    fn events(&self) -> Sse<BoxStream<'static, Event>> {
        Sse::new(Box::pin(stream::iter([Event::default().data("one")])))
    }
}

/// A generic controller.
#[injectable]
pub struct Generic<T: Send + Sync + 'static> {
    #[injectable(default)]
    pub marker: PhantomData<fn() -> T>,
}

#[routes]
impl<T: Send + Sync + 'static> Generic<T> {
    #[ulo_http::get("/generic")]
    fn generic(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

/// A controller declared inside a fn body, whose markers repeat, carry a path, follow the
/// transport attribute and sit inside `cfg_attr`.
pub fn local() {
    #[injectable]
    struct Local {
        #[injectable(default)]
        a: u8,
        #[ulo::injectable(default)]
        b: u8,
    }

    #[routes]
    #[guards(value = Allow)]
    #[ulo::guards(value = Allow)]
    impl Local {
        #[guards(value = Allow)]
        #[ulo_http::get("/local")]
        #[ulo::meta(Tag("local"))]
        #[cfg_attr(all(), meta(Tag("gated")), interceptors(value = Pass))]
        fn local(&self) -> String {
            format!("{}{}", self.a, self.b)
        }
    }
}

/// Writes a controller whose markers reach `#[routes]` spanned in this macro's body, a span local to
/// the crate, so a lint the generated code trips is reported here as it is in hand-written code.
macro_rules! controller {
    ($name:ident, $path:literal) => {
        #[injectable]
        pub struct $name;

        #[routes]
        #[guards(value = Allow)]
        #[meta(Tag("generated"))]
        impl $name {
            #[ulo_http::get($path)]
            #[guards(value = Allow)]
            #[interceptors(value = Pass)]
            fn handle(&self) -> &'static str {
                $path
            }
        }
    };
}

controller!(GeneratedA, "/generated-a");
controller!(GeneratedB, "/generated-b");
