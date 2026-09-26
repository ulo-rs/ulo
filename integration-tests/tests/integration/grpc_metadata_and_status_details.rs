//! gRPC metadata reaches an application whole, and an error's detail reaches the caller.
//!
//! A repeated ASCII key keeps every value and a `-bin` key keeps its bytes, for a guard as for
//! the handler. An error carrying `details()` answers with `grpc-status-details-bin`: a
//! `google.rpc.Status` repeating the code and message, with the detail as one `Any`.

#![allow(dead_code)]

use crate::common::NotServed;
use prost::Message;
use serial_test::serial;
use ulo::UloFactory;
use ulo::context::ExecutionContext;
use ulo::extract::Payload;
use ulo::grpc::GrpcContext;
use ulo::grpc::extract::Inbound;
use ulo::{ErrorKind, async_trait, injectable, module};
use ulo_macros::{controller, grpc_methods, new, use_guards};

mod details_pb {
    tonic::include_proto!("ulo_test.orders");
}

use details_pb::orders_client::OrdersClient;
use details_pb::orders_server::{Orders, OrdersServer};

/// A failure whose detail is whatever the request's item names.
#[derive(Debug)]
struct Rejected {
    details: Option<serde_json::Value>,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("rejected")
    }
}

impl std::error::Error for Rejected {}

impl ulo::Error for Rejected {
    fn kind(&self) -> ErrorKind {
        ErrorKind::BadRequest
    }

    fn details(&self) -> Option<serde_json::Value> {
        self.details.clone()
    }
}

fn detail_for(item: &str) -> Option<serde_json::Value> {
    match item {
        "object" => Some(serde_json::json!({"field": "qty", "min": 1, "tags": ["a", true, null]})),
        "array" => Some(serde_json::json!(["qty", 1])),
        _ => None,
    }
}

/// What the guard read, handed to the handler through the execution's bag.
#[derive(Clone)]
struct SeenByGuard(Option<Vec<u8>>);

#[injectable]
pub struct ReadsBinary {}

#[async_trait]
impl ulo::enhancer::Guard<GrpcContext> for ReadsBinary {
    async fn can_activate(&self, ctx: &GrpcContext) -> bool {
        let seen = ctx.header_bin("x-trace-bin").map(<[u8]>::to_vec);
        ctx.extensions().insert(SeenByGuard(seen));
        true
    }
}

#[controller]
pub struct MetadataService {}

#[grpc_methods(details_pb::orders_server::Orders)]
#[use_guards(ReadsBinary)]
impl MetadataService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// `report` answers with what the call carried; any other item fails with its detail.
    #[grpc_method]
    async fn create(
        &self,
        ctx: &GrpcContext,
        Payload(req): Payload<details_pb::CreateOrderRequest>,
    ) -> Result<details_pb::CreateOrderResponse, Rejected> {
        if req.item != "report" {
            return Err(Rejected {
                details: detail_for(&req.item),
            });
        }
        let guard = ctx.extensions().get::<SeenByGuard>().and_then(|s| s.0);
        Ok(details_pb::CreateOrderResponse {
            id: 0,
            status: format!(
                "all={:?} last={:?} bin={:?} guard={:?}",
                ctx.headers_all("x-tag").collect::<Vec<_>>(),
                ctx.header("x-tag"),
                ctx.header_bin("x-trace-bin"),
                guard,
            ),
        })
    }

    /// One item, failing with the object detail.
    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<details_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<details_pb::ProgressEvent, Rejected>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::iter([Err(Rejected {
            details: detail_for("object"),
        })]))
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<details_pb::CreateOrderRequest>,
    ) -> Result<details_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<details_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<details_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [MetadataService], providers: [ReadsBinary])]
impl MetadataModule {}

async fn client() -> OrdersClient<tonic::transport::Channel> {
    let adapter = ulo_grpc::GrpcAdapter::new("127.0.0.1:0".parse().unwrap());
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(MetadataModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    let port = port_rx.await.unwrap();
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    )
}

fn create(item: &str) -> tonic::Request<details_pb::CreateOrderRequest> {
    tonic::Request::new(details_pb::CreateOrderRequest {
        item: item.to_string(),
        qty: 0,
    })
}

async fn report(request: tonic::Request<details_pb::CreateOrderRequest>) -> String {
    client()
        .await
        .create(request)
        .await
        .expect("report answers")
        .into_inner()
        .status
}

/// The trailer's `google.rpc.Status`, decoded.
fn trailer(status: &tonic::Status) -> tonic_types::Status {
    tonic_types::Status::decode(status.details()).expect("a google.rpc.Status")
}

#[serial]
#[tokio::test]
async fn a_repeated_key_keeps_every_value_and_a_binary_key_its_bytes() {
    let mut request = create("report");
    let metadata = request.metadata_mut();
    metadata.append("x-tag", "first".parse().unwrap());
    metadata.append("x-tag", "second".parse().unwrap());
    metadata.insert_bin(
        "x-trace-bin",
        tonic::metadata::MetadataValue::from_bytes(&[0, 159, 255]),
    );

    assert_eq!(
        report(request).await,
        r#"all=["first", "second"] last=Some("second") bin=Some([0, 159, 255]) guard=Some([0, 159, 255])"#
    );
}

#[serial]
#[tokio::test]
async fn a_call_carrying_neither_reads_as_absent() {
    assert_eq!(
        report(create("report")).await,
        "all=[] last=None bin=None guard=None"
    );
}

#[serial]
#[tokio::test]
async fn an_object_detail_travels_as_a_struct() {
    let status = client().await.create(create("object")).await.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    let trailer = trailer(&status);
    assert_eq!(trailer.code, tonic::Code::InvalidArgument as i32);
    assert_eq!(trailer.message, "rejected");
    assert_eq!(trailer.details.len(), 1);
    assert_eq!(
        trailer.details[0].type_url,
        "type.googleapis.com/google.protobuf.Struct"
    );
    let detail = prost_types::Struct::decode(trailer.details[0].value.as_slice()).unwrap();

    use prost_types::value::Kind;
    let field = |name: &str| detail.fields[name].kind.clone().unwrap();
    assert_eq!(field("field"), Kind::StringValue("qty".into()));
    assert_eq!(field("min"), Kind::NumberValue(1.0));
    let Kind::ListValue(tags) = field("tags") else {
        panic!("tags is a list");
    };
    let tags: Vec<Kind> = tags.values.into_iter().map(|v| v.kind.unwrap()).collect();
    assert_eq!(
        tags,
        [
            Kind::StringValue("a".into()),
            Kind::BoolValue(true),
            Kind::NullValue(0),
        ]
    );
}

#[serial]
#[tokio::test]
async fn a_detail_that_is_not_an_object_travels_as_a_value() {
    let status = client().await.create(create("array")).await.unwrap_err();

    let trailer = trailer(&status);
    assert_eq!(
        trailer.details[0].type_url,
        "type.googleapis.com/google.protobuf.Value"
    );
    let detail = prost_types::Value::decode(trailer.details[0].value.as_slice()).unwrap();
    let Some(prost_types::value::Kind::ListValue(items)) = detail.kind else {
        panic!("a list: {detail:?}");
    };
    assert_eq!(items.values.len(), 2);
}

#[serial]
#[tokio::test]
async fn an_error_without_detail_writes_no_trailer() {
    let status = client().await.create(create("plain")).await.unwrap_err();

    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.details().is_empty(), "{:?}", status.details());
}

#[serial]
#[tokio::test]
async fn a_failing_stream_item_carries_its_detail() {
    let mut stream = client()
        .await
        .watch_progress(details_pb::WatchRequest { id: 1 })
        .await
        .expect("the stream opens")
        .into_inner();

    let status = stream
        .message()
        .await
        .expect_err("the item fails the stream");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert_eq!(
        trailer(&status).details[0].type_url,
        "type.googleapis.com/google.protobuf.Struct"
    );
}
