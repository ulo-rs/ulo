//! A call compressed with an encoding the server does not decompress answers UNIMPLEMENTED with
//! `grpc-accept-encoding: identity`, which the gRPC compression specification prescribes, through
//! the screen that keeps every reply's `grpc-` headers as the framework wrote them.
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

#[tokio::test]
async fn a_call_in_an_encoding_the_server_does_not_decompress_is_told_the_one_it_accepts() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let (client, _) = app.plain_http2();
    let request = Request::post(format!("{}/codegen.probe.v1.Known/Upper", app.origin()))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("grpc-encoding", "gzip")
        .body(tonic::body::Body::new(Full::new(Bytes::from_static(&[1, 0, 0, 0, 0]))))
        .unwrap_or_else(|error| panic!("bad request: {error}"));
    let reply = support::within("the reply's head", client.request(request)).await.unwrap_or_else(|error| panic!("the call failed: {error}"));
    let headers = reply.headers();
    assert_eq!(headers.get("grpc-status").map(|value| value.as_bytes()), Some(&b"12"[..]), "{headers:?}");
    assert_eq!(headers.get("grpc-accept-encoding").map(|value| value.as_bytes()), Some(&b"identity"[..]), "{headers:?}");
    assert!(capture.warnings().is_empty(), "the framework's own header read as an entry's: {:?}", capture.warnings());
    app.stop().await;
}
