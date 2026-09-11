// Tests: conformance with the HTTP adapter SPI is proved once for all five
// adapters in `integration-tests` — the four `*_conformance` suites, each
// instantiated per adapter. This crate's `tests/` covers only what is
// actix's: collecting bodies in both directions, and the 256 KiB payload
// ceiling that follows from it. actix serves no WebSocket at all.

//! # ulo-http-actix
//!
//! Actix-web adapter for the Ulo framework.
//!
//! This crate provides an implementation of Ulo's `HttpAdapter` trait for the Actix-web framework,
//! allowing you to use Actix-web as the HTTP server for your Ulo applications.
//!
//! ## Usage
//!
//! ```ignore
//! use ulo_http_actix::ActixAdapter;
//!
//! #[actix_web::main]
//! async fn main() {
//!     let mut app = UloFactory::new()
//!         .create_with(AppModule)
//!         .await
//!         .unwrap();
//!     app.use_http_adapter(ActixAdapter::new(), ("127.0.0.1", 3000)).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```

mod actix_adapter;

pub use actix_adapter::ActixAdapter;

pub use ulo::HttpAdapter;
