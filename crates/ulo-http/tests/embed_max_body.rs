//! `Embedded::max_body` is the body limit of a route that declares none: a body over it is refused
//! 413, and one at it is read.

use ulo::{App, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Service};
use ulo_http::{Bytes, ConnInfo, HttpBody, Request, Response};
use ulo_runtime_tokio::Tokio;

struct Host;

impl Embed for Host {
    const NAME: &'static str = "test-host";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }
}

#[controller]
struct Upload;

#[handlers]
impl Upload {
    #[ulo_http::post("/bytes")]
    fn bytes(&self, bytes: Bytes) -> String {
        bytes.len().to_string()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Upload>();
    }
}

async fn post(service: &Service<Host>, body: &'static str) -> Response {
    let (head, ()) = http::Request::post("/bytes").body(()).expect("a request").into_parts();
    let req = Request { head: head.clone(), body: HttpBody::from(body), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
    service.respond(&head, req).await
}

#[tokio::test]
async fn a_body_over_the_embedding_s_max_body_is_refused_413() {
    let server = Embedded::<Host>::new().max_body(8);
    let handle = server.handle();
    let _bound = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens");
    let service = handle.service();

    assert_eq!(post(&service, "12345678").await.status(), 200, "a body at the limit is read");
    assert_eq!(post(&service, "123456789").await.status(), 413, "a body over the limit is refused");
}
