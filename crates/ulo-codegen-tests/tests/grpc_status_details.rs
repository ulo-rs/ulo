//! The `google.rpc.Status` an error's details travel in, in `grpc-status-details-bin` (transports
//! DESIGN §2.4, §6.2): `ErrorInfo`, `BadRequest`, `Help` and `RetryInfo`, a `Json` detail as a
//! `google.protobuf.Value` in an `Any`, no trailer for an error without details, and the code an
//! error is sent with. Capability: gRPC `Details` (G19).

mod support;
mod wire;

use prost::Message as _;
use tonic::Code;
use tonic_types::StatusExt;
use ulo_codegen_tests::pb::AddRequest;
use ulo_codegen_tests::pb::counter_client::CounterClient;

use support::Running;
use wire::{Hold, Root};

async fn fail(a: i64, b: i64) -> tonic::Status {
    let app = Running::start(Root(Hold::new()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = CounterClient::new(app.channel().await);
    let status = match client.add(AddRequest { a, b }).await {
        Err(status) => status,
        Ok(reply) => panic!("Add({a}, {b}) answered {:?}", reply.into_inner()),
    };
    app.stop().await;
    status
}

#[tokio::test]
async fn an_error_info_detail_arrives_with_its_reason_domain_and_metadata_under_the_kind_s_code() {
    let status = fail(-1, 0).await;
    assert_eq!(status.code(), Code::Aborted, "Conflict maps to ABORTED");
    assert_eq!(status.message(), "taken");
    let details = status.get_error_details();
    let info = details.error_info().unwrap_or_else(|| panic!("no ErrorInfo in {details:?}"));
    assert_eq!(info.reason, "NAME_TAKEN");
    assert_eq!(info.domain, "counter.example");
    assert_eq!(info.metadata.get("name").map(String::as_str), Some("x"));
    assert!(details.bad_request().is_none() && details.retry_info().is_none() && details.help().is_none());
}

#[tokio::test]
async fn violations_help_and_retry_after_arrive_as_one_bad_request_one_help_and_one_retry_info() {
    let status = fail(-2, 0).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    let details = status.get_error_details();
    let bad = details.bad_request().unwrap_or_else(|| panic!("no BadRequest in {details:?}"));
    let fields: Vec<(&str, &str)> = bad.field_violations.iter().map(|v| (v.field.as_str(), v.description.as_str())).collect();
    assert_eq!(fields, vec![("a", "negative"), ("b", "too large")], "two FieldViolations details write into the one BadRequest");
    let help = details.help().unwrap_or_else(|| panic!("no Help in {details:?}"));
    assert_eq!(help.links.len(), 1);
    assert_eq!(help.links[0].url, "https://example.com/rules");
    assert_eq!(help.links[0].description, "the rules");
    let retry = details.retry_info().unwrap_or_else(|| panic!("no RetryInfo in {details:?}"));
    assert_eq!(retry.retry_delay, Some(std::time::Duration::from_secs(30)));
}

#[tokio::test]
async fn the_first_error_info_of_two_is_the_one_sent() {
    let status = fail(-6, 0).await;
    let details = status.get_error_details();
    let info = details.error_info().unwrap_or_else(|| panic!("no ErrorInfo in {details:?}"));
    assert_eq!((info.reason.as_str(), info.domain.as_str()), ("FIRST", "d1"));
}

#[tokio::test]
async fn a_json_detail_is_a_protobuf_value_packed_in_an_any() {
    let status = fail(-3, 0).await;
    assert_eq!(status.code(), Code::NotFound);
    let packed = tonic_types::Status::decode(status.details()).unwrap_or_else(|error| panic!("details are not a google.rpc.Status: {error}"));
    assert_eq!(packed.code, i32::from(Code::NotFound), "the Status inside repeats the code");
    assert_eq!(packed.message, "missing", "and the message");
    assert_eq!(packed.details.len(), 1);
    let any = &packed.details[0];
    assert_eq!(any.type_url, "type.googleapis.com/google.protobuf.Value");
    let value = prost_types::Value::decode(any.value.as_slice()).unwrap_or_else(|error| panic!("the Any is not a Value: {error}"));
    let Some(prost_types::value::Kind::StructValue(object)) = value.kind else {
        panic!("a JSON object is a Struct, got {value:?}");
    };
    assert_eq!(object.fields.len(), 2);
    assert!(matches!(&object.fields["s"].kind, Some(prost_types::value::Kind::StringValue(text)) if text == "text"));
    let Some(prost_types::value::Kind::ListValue(list)) = &object.fields["k"].kind else {
        panic!("an array is a ListValue, got {:?}", object.fields["k"]);
    };
    let kinds: Vec<_> = list.values.iter().map(|v| v.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            Some(prost_types::value::Kind::NumberValue(1.0)),
            Some(prost_types::value::Kind::BoolValue(true)),
            Some(prost_types::value::Kind::NullValue(0)),
        ]
    );
}

#[tokio::test]
async fn an_error_without_details_sends_no_details_trailer() {
    let status = fail(-4, 0).await;
    assert_eq!(status.code(), Code::NotFound);
    assert!(status.details().is_empty(), "grpc-status-details-bin was sent: {:?}", status.details());
    assert!(status.metadata().get_bin("grpc-status-details-bin").is_none());
}

#[tokio::test]
async fn an_error_can_name_its_grpc_code_and_a_code_outside_one_to_sixteen_is_ignored() {
    assert_eq!(fail(-5, 6).await.code(), Code::AlreadyExists, "with_grpc_code(6)");
    assert_eq!(fail(-5, 99).await.code(), Code::Aborted, "99 is outside 1..=16: the kind's code");
    assert_eq!(fail(-5, 0).await.code(), Code::Aborted, "OK is not an error code: the kind's code");
}
