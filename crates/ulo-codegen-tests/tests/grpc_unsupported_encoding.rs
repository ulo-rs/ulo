//! A call compressed in an encoding the server does not decompress answers UNIMPLEMENTED with
//! `grpc-accept-encoding` naming the encodings it does, `identity` last, which the gRPC compression
//! specification prescribes, through the screen that keeps every reply's `grpc-` headers as the
//! framework wrote them: on a handler's path, the health service's and the reflection service's,
//! at the default `accept_encodings` and with gzip turned off.
//!
//! In a file of its own: the screen's once-per-process warning record would otherwise depend on
//! which test of the binary was screened first.

mod support;

use http::Request;
use http_body_util::Full;
use prost::bytes::Bytes;
use ulo::{Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_codegen_tests::probe;
use ulo_grpc::Message;

use support::{Capture, Running};

#[controller]
struct Known;

#[handlers]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Known>();
    }
}

/// The status and `grpc-accept-encoding` a call to `path` compressed in `encoding` is answered
/// with: the trailers-only reply's head carries both.
async fn refusal(app: &Running, path: &str, encoding: &str) -> (Option<String>, Option<String>) {
    let (client, _) = app.plain_http2();
    let request = Request::post(format!("{}{path}", app.origin()))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("grpc-encoding", encoding)
        .body(tonic::body::Body::new(Full::new(Bytes::from_static(&[1, 0, 0, 0, 0]))))
        .unwrap_or_else(|error| panic!("bad request: {error}"));
    let reply = support::within("the reply's head", client.request(request)).await.unwrap_or_else(|error| panic!("the call failed: {error}"));
    let header = |name: &str| reply.headers().get(name).map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    (header("grpc-status"), header("grpc-accept-encoding"))
}

/// Planted breaks: `Encodings::finish` writing no `grpc-accept-encoding` fails the handler path's
/// assertion; the health or reflection service built without `accept_compressed` names `identity`
/// alone in its own refusal of `zstd`.
#[tokio::test]
async fn a_call_in_an_encoding_the_server_does_not_decompress_is_told_the_ones_it_accepts() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let upper = "/codegen.probe.v1.Known/Upper";
    let health = "/grpc.health.v1.Health/Check";
    let reflection = "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo";

    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(true)).await;
    for path in [upper, health, reflection] {
        let (status, accepted) = refusal(&app, path, "zstd").await;
        assert_eq!((status.as_deref(), accepted.as_deref()), (Some("12"), Some("gzip,identity")), "{path}");
    }
    app.stop().await;

    let off = ulo_grpc::Server::new("127.0.0.1:0").reflection(true).accept_encodings([]);
    let app = Running::start(Root, off).await;
    for path in [upper, health, reflection] {
        let (status, accepted) = refusal(&app, path, "gzip").await;
        assert_eq!((status.as_deref(), accepted.as_deref()), (Some("12"), Some("identity")), "{path}");
    }
    app.stop().await;
    assert!(capture.warnings().is_empty(), "the framework's own header read as an entry's: {:?}", capture.warnings());
}
