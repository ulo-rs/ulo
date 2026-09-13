//! Conformance suite for trailing-slash-insensitive route matching.
//!
//! `/app` and `/app/` address the same route on every HTTP adapter:
//! [`AdapterContext`] trims trailing slashes from the request path before the
//! global chain and the native router see it, and `join_route` guarantees
//! registered paths never carry one. The query string survives trimming, and
//! the root path `/` is preserved.
//!
//! [`AdapterContext`]: ulo::spi::AdapterContext

use crate::common::TestServer;
use serde::Deserialize;
use ulo::http::Body;
use ulo::http::extract::{Path, Query};
use ulo::{UloFactory, controller, get, module, routes};
#[derive(Deserialize)]
struct EchoParams {
    name: String,
}

#[controller("/app")]
pub struct AppController {}

#[routes]
impl AppController {
    #[get("/")]
    fn root(&self) -> Body {
        Body::text("root")
    }

    #[get("/user/{id}")]
    fn user(&self, Path(id): Path<u32>) -> Body {
        Body::text(format!("user:{id}"))
    }

    #[get("/echo")]
    fn echo(&self, Query(params): Query<EchoParams>) -> Body {
        Body::text(params.name)
    }

    #[get("/slashed/")]
    fn slashed(&self) -> Body {
        Body::text("slashed")
    }
}

#[controller("/")]
pub struct RootController {}

#[routes]
impl RootController {
    #[get("/")]
    fn index(&self) -> Body {
        Body::text("index")
    }
}

#[module(controllers: [AppController, RootController])]
impl TrailingSlashModule {}

async fn boot(adapter: impl ulo::http::HttpAdapter + 'static) -> TestServer {
    TestServer::start_adapter(UloFactory::new(), TrailingSlashModule, adapter).await
}

async fn expect_text(server: &TestServer, path: &str, body: &str) {
    let resp = server.client().get(server.url(path)).send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, 200, "GET {path} → {text}");
    assert_eq!(text, body, "GET {path}");
}

async fn case_controller_root(server: TestServer) {
    expect_text(&server, "/app", "root").await;
    expect_text(&server, "/app/", "root").await;
}

async fn case_nested_and_params(server: TestServer) {
    expect_text(&server, "/app/user/5", "user:5").await;
    expect_text(&server, "/app/user/5/", "user:5").await;
}

async fn case_query_survives(server: TestServer) {
    expect_text(&server, "/app/echo?name=a", "a").await;
    expect_text(&server, "/app/echo/?name=b", "b").await;
}

async fn case_declared_with_trailing_slash(server: TestServer) {
    expect_text(&server, "/app/slashed", "slashed").await;
    expect_text(&server, "/app/slashed/", "slashed").await;
}

async fn case_root_path_preserved(server: TestServer) {
    expect_text(&server, "/", "index").await;
}

macro_rules! trailing_slash_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio_localset_test::localset_test]
            async fn controller_root_matches_both_forms() {
                super::case_controller_root(super::boot($adapter).await).await;
            }

            #[tokio_localset_test::localset_test]
            async fn nested_and_param_paths_match_both_forms() {
                super::case_nested_and_params(super::boot($adapter).await).await;
            }

            #[tokio_localset_test::localset_test]
            async fn query_survives_trimming() {
                super::case_query_survives(super::boot($adapter).await).await;
            }

            #[tokio_localset_test::localset_test]
            async fn declared_trailing_slash_matches_both_forms() {
                super::case_declared_with_trailing_slash(super::boot($adapter).await).await;
            }

            #[tokio_localset_test::localset_test]
            async fn root_path_preserved() {
                super::case_root_path_preserved(super::boot($adapter).await).await;
            }
        }
    };
}

trailing_slash_suite!(axum, ulo_http_axum::AxumAdapter::new());
trailing_slash_suite!(poem, ulo_http_poem::PoemAdapter::new());
trailing_slash_suite!(salvo, ulo_http_salvo::SalvoAdapter::new());
trailing_slash_suite!(actix, ulo_http_actix::ActixAdapter::new());
trailing_slash_suite!(rocket, ulo_http_rocket::RocketAdapter::new());
