// Tests: this crate has no `tests/` of its own. Nothing it does is
// observable without an application dispatching through it, so its
// behaviour is proved in `integration-tests`:
// `ws_listener_adoption.rs`, and the separate-port half of the `ws_*` files.

//! tokio-tungstenite adapter for standalone WebSocket deployment with the Ulo framework.
//!
//! Provides `TungsteniteAdapter`, which implements `WsAdapter` for separate-port
//! WebSocket servers — gateways that declare `port = N` in the `#[websocket_gateway]` macro
//! are routed here instead of through the HTTP adapter.
//!
//! # Example
//!
//! ```rust,ignore
//! app.use_websocket_adapter(TungsteniteAdapter::new()).unwrap();
//! app.start().await.unwrap();
//! // Gateways with port = 4000 automatically bind to 4000 via TungsteniteAdapter.
//! ```

mod adapter;

pub use adapter::TungsteniteAdapter;
