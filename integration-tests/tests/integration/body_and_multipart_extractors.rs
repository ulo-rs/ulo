//! `Body<T>` reads a typed body by its content type, and `Multipart` reads a multipart form
//! part by part.
//!
//! `Body<T>` has one arm per case: JSON, a urlencoded form, no content type (tried as JSON, then
//! as a form), and any other content type, refused. `Multipart` hands the body to `multer` as a
//! stream, with the boundary taken from the header; one round trip with a text part and a file
//! part covers that hand-off.

use serde::{Deserialize, Serialize};
use ulo::http::Body as HttpBody;
use ulo::http::extract::{Body, Multipart};
use ulo::{controller, module, new, post, routes};

use crate::common::TestServer;

#[derive(Deserialize, Serialize)]
struct Order {
    item: String,
    qty: u32,
}

#[controller("/extract")]
pub struct ExtractController {}

#[routes]
impl ExtractController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[post("/typed")]
    async fn typed(&self, Body(order): Body<Order>) -> HttpBody {
        HttpBody::json(serde_json::json!({ "item": order.item, "qty": order.qty }))
    }

    #[post("/upload")]
    async fn upload(&self, mut form: Multipart) -> HttpBody {
        let mut description = String::new();
        let mut file_name = String::new();
        let mut bytes = Vec::new();
        while let Some(field) = form.next_field().await.unwrap() {
            match field.name() {
                Some("description") => description = field.text().await.unwrap(),
                Some("file") => {
                    file_name = field.file_name().unwrap_or("").to_string();
                    bytes = field.bytes().await.unwrap().to_vec();
                }
                _ => {}
            }
        }
        HttpBody::json(serde_json::json!({
            "description": description,
            "file": file_name,
            "bytes": bytes,
        }))
    }
}

#[module(controllers: [ExtractController])]
impl ExtractModule {}

async fn typed(content_type: Option<&str>, body: &'static str) -> reqwest::Response {
    let server = TestServer::start(ExtractModule).await;
    let mut req = server
        .client()
        .post(server.url("/extract/typed"))
        .body(body);
    if let Some(ct) = content_type {
        req = req.header("content-type", ct);
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn a_json_body_is_read_as_json() {
    let resp = typed(Some("application/json"), r#"{"item":"keyboard","qty":2}"#).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["item"], "keyboard");
    assert_eq!(body["qty"], 2);
}

#[tokio::test]
async fn a_urlencoded_body_is_read_as_a_form() {
    let resp = typed(
        Some("application/x-www-form-urlencoded"),
        "item=keyboard&qty=2",
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["item"], "keyboard");
    assert_eq!(body["qty"], 2);
}

#[tokio::test]
async fn a_body_with_no_content_type_is_read_as_either() {
    for (bytes, what) in [
        (r#"{"item":"keyboard","qty":2}"#, "JSON bytes"),
        ("item=keyboard&qty=2", "form bytes"),
    ] {
        let resp = typed(None, bytes).await;
        assert_eq!(resp.status(), 200, "{what} with no content type");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["item"], "keyboard", "{what} with no content type");
    }
}

#[tokio::test]
async fn an_unsupported_content_type_is_refused() {
    let resp = typed(Some("text/csv"), "item,qty\nkeyboard,2").await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["details"]
            .as_str()
            .unwrap_or("")
            .contains("Unsupported content type: text/csv"),
        "the refusal names the content type: {body}"
    );
}

/// Written by hand because reqwest's `multipart` feature is off here, and so the bytes on the
/// wire do not depend on a client's encoder.
#[tokio::test]
async fn a_multipart_form_round_trips_a_text_part_and_a_file_part() {
    let server = TestServer::start(ExtractModule).await;
    let boundary = "ulo-boundary-7f3a";
    let payload = [1u8, 2, 3, 4, 5, 250, 251];
    let mut form = Vec::new();
    form.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nan invoice\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"scan.bin\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    form.extend_from_slice(&payload);
    form.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = server
        .client()
        .post(server.url("/extract/upload"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["description"], "an invoice");
    assert_eq!(body["file"], "scan.bin");
    assert_eq!(body["bytes"], serde_json::json!(payload));
}
