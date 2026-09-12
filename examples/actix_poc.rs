//! ulo-http-actix proof-of-concept
//!
//! What this adapter does that the other four do not: it collects request and
//! response bodies in full before either side sees them. A `BodyStream`
//! handler still works and receives the whole body as one chunk, streaming
//! responses do not, and actix-web's `PayloadConfig` caps a request at 256 KiB
//! — over that is a 413 before any handler runs, and the adapter surfaces no
//! knob to raise it.
//!
//! Actix serves no WebSocket, so unlike the salvo, poem and rocket
//! proof-of-concepts there is no gateway here.
//!
//! Run with: cargo run --example actix_poc
//! Test:     curl http://127.0.0.1:3000/hello
//!           curl http://127.0.0.1:3000/hello/world
//!           curl -X POST --data-binary @- http://127.0.0.1:3000/hello/count < /dev/urandom
//!           # a body over 256 KiB answers 413:
//!           head -c 300000 /dev/zero | curl -X POST --data-binary @- \
//!               http://127.0.0.1:3000/hello/count -o /dev/null -w '%{http_code}\n'

use futures::StreamExt;
use serde_json::json;
use ulo::http::extract::{BodyStream, Bytes, Path};
use ulo::*;
use ulo_http_actix::ActixAdapter;
use ulo_macros::module;

#[controller("/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    fn hello(&self) -> Body {
        Body::text("hello from actix")
    }

    #[get("/{name}")]
    fn greet(&self, name: Path<String>) -> Body {
        Body::json(json!({ "greeting": format!("hello, {}", name.0) }))
    }

    #[post("/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::text(format!("read {} bytes", body.0.len()))
    }

    /// The handler signature says streaming; the adapter has already collected
    /// the payload, so `chunks` is 1 for a body of any size it accepts.
    #[post("/count")]
    async fn count(&self, body: BodyStream) -> Body {
        let mut total = 0u64;
        let mut chunks = 0u32;
        let mut stream = Box::pin(body.into_stream());
        while let Some(chunk) = stream.next().await {
            if let Ok(bytes) = chunk {
                total += bytes.len() as u64;
                chunks += 1;
            }
        }
        Body::json(json!({ "bytes": total, "chunks": chunks }))
    }
}

#[module(controllers: [HelloController])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;
    app.use_http_adapter(ActixAdapter::new(), ("127.0.0.1", 3000))?;
    println!("listening on http://127.0.0.1:3000");
    app.start().await?;
    Ok(())
}
