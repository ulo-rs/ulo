// Tests: conformance with the HTTP adapter SPI is proved once for all five
// adapters in `integration-tests` — the `*_conformance` suites, each
// instantiated per adapter. This crate's `tests/` covers only what is
// axum's: body streaming, and WebSocket upgrade on both port modes.

//! # ulo-http-axum
//!
//! Axum adapter for the Ulo framework.
//!
//! This crate provides an implementation of Ulo's `HttpAdapter` and `WsAdapter` traits
//! for the Axum web framework.
//!
//! ## Usage
//!
//! ```ignore
//! use ulo_http_axum::AxumAdapter;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = UloFactory::new()
//!         .create_with(AppModule)
//!         .await
//!         .unwrap();
//!     app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000)).unwrap();
//!     // Only needed for `#[websocket_gateway(port = N)]` gateways.
//!     app.use_websocket_adapter(AxumAdapter::new()).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```

mod axum_adapter;
mod axum_websocket_adapter;
pub(crate) mod tokio_sender;

pub use axum_adapter::AxumAdapter;
pub use tokio_sender::TokioSender;

pub use ulo::http::HttpAdapter;

pub use ulo::ws::WsAdapter;
