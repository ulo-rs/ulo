use ex_29_grpc_service::pb::catalog_client::CatalogClient;
use ex_29_grpc_service::pb::{GetItemRequest, Item, ListItemsRequest};
use ex_29_grpc_service::{TOKEN, builder};
use ex_support::{Served, serve};
use prost::Message as _;
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Request, Status};
use tonic_types::StatusExt;
use ulo_grpc::Server;
use ulo_runtime_tokio::Tokio;

async fn start() -> (Served, CatalogClient<Channel>) {
    let app = builder()
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    let served = serve(app);
    let channel = Endpoint::from_shared(format!("http://{}", served.addr)).expect("an endpoint").connect().await.expect("the client connects");
    (served, CatalogClient::new(channel))
}

fn authorized<T>(message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", TOKEN.parse().expect("ascii metadata"));
    request
}

fn failed<T: std::fmt::Debug>(reply: Result<tonic::Response<T>, Status>) -> Status {
    reply.expect_err("the call should fail")
}

#[tokio::test]
async fn a_unary_call_answers_the_item() {
    let (served, mut client) = start().await;
    let reply = client.get_item(authorized(GetItemRequest { id: "b2".into() })).await.expect("GetItem answers");
    assert_eq!(reply.into_inner(), Item { id: "b2".into(), name: "bellows".into() });
    served.stop().await.expect("the app closes");
}

#[tokio::test]
async fn a_server_streaming_call_answers_each_item_in_order() {
    let (served, mut client) = start().await;
    let mut stream = client.list_items(authorized(ListItemsRequest { limit: 2 })).await.expect("ListItems answers").into_inner();
    let mut ids = Vec::new();
    while let Some(item) = stream.message().await.expect("the stream reads") {
        ids.push((item.id, item.name));
    }
    assert_eq!(ids, [("a1".to_owned(), "anvil".to_owned()), ("b2".to_owned(), "bellows".to_owned())]);
    served.stop().await.expect("the app closes");
}

#[tokio::test]
async fn the_guard_answers_unauthenticated_to_a_missing_or_wrong_token_on_both_methods() {
    let (served, mut client) = start().await;
    let missing = failed(client.get_item(GetItemRequest { id: "a1".into() }).await);
    assert_eq!((missing.code(), missing.message()), (Code::Unauthenticated, "unauthorized"));

    let mut wrong = Request::new(GetItemRequest { id: "a1".into() });
    wrong.metadata_mut().insert("authorization", "Bearer closed".parse().expect("ascii metadata"));
    assert_eq!(failed(client.get_item(wrong).await).code(), Code::Unauthenticated);

    let streamed = failed(client.list_items(ListItemsRequest { limit: 1 }).await);
    assert_eq!(streamed.code(), Code::Unauthenticated);
    served.stop().await.expect("the app closes");
}

/// The type URL of every `Any` the status's `google.rpc.Status` packs.
fn packed_types(status: &Status) -> Vec<String> {
    let packed = tonic_types::Status::decode(status.details()).expect("grpc-status-details-bin is a google.rpc.Status");
    packed.details.into_iter().map(|any| any.type_url).collect()
}

#[tokio::test]
async fn a_missing_item_is_not_found_with_an_error_info_naming_it() {
    let (served, mut client) = start().await;
    let status = failed(client.get_item(authorized(GetItemRequest { id: "zz".into() })).await);
    assert_eq!((status.code(), status.message()), (Code::NotFound, "no item `zz`"));
    let details = status.get_error_details();
    let info = details.error_info().expect("an ErrorInfo detail");
    assert_eq!((info.reason.as_str(), info.domain.as_str()), ("ITEM_NOT_FOUND", "catalog.example"));
    assert_eq!(info.metadata.get("id").map(String::as_str), Some("zz"));
    assert_eq!(packed_types(&status), ["type.googleapis.com/google.rpc.ErrorInfo"]);
    served.stop().await.expect("the app closes");
}

#[tokio::test]
async fn an_empty_id_is_invalid_argument_with_the_field_violation() {
    let (served, mut client) = start().await;
    let status = failed(client.get_item(authorized(GetItemRequest { id: String::new() })).await);
    assert_eq!((status.code(), status.message()), (Code::InvalidArgument, "an item id is required"));
    let details = status.get_error_details();
    let bad = details.bad_request().expect("a BadRequest detail");
    let fields: Vec<_> = bad.field_violations.iter().map(|v| (v.field.as_str(), v.description.as_str())).collect();
    assert_eq!(fields, [("id", "must not be empty")]);
    assert_eq!(packed_types(&status), ["type.googleapis.com/google.rpc.BadRequest"]);
    served.stop().await.expect("the app closes");
}
