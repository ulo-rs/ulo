//! The minimal ulo HTTP application
//!
//! Starting point for anyone new to the framework. Shows the three things
//! every ulo app needs: a controller, a module, and an adapter.
//!
//! Run with:  cargo run --example hello_world
//! Test:      curl http://127.0.0.1:3000/hello
//!            curl http://127.0.0.1:3000/hello/json

use serde_json::json;
use ulo::http::Body;
use ulo::prelude::*;
use ulo_http_axum::AxumAdapter;

#[controller("/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    fn hello(&self) -> Body {
        Body::text("Hello, World!".to_string())
    }

    #[get("/json")]
    fn hello_json(&self) -> Body {
        Body::json(json!({
            "message": "Hello, World!",
            "framework": "ulo"
        }))
    }
}

#[module(controllers: [HelloController], providers: [])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 ulo hello world\n");
    println!("  GET http://127.0.0.1:3000/hello");
    println!("  GET http://127.0.0.1:3000/hello/json");
    println!();

    let mut app = UloFactory::new().create_with(AppModule).await?;

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();

    app.start().await?;
    Ok(())
}
