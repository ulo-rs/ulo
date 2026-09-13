// ADR-0031: one transport per struct. A second handler impl collides on the
// generated dispatch entry rather than quietly serving two protocols.
use ulo::{controller, get, patterns, routes};
use ulo::http::Body;
use ulo::rpc::RpcContext;
use ulo::rpc::{RpcData, RpcError};

#[controller("/p")]
pub struct Both {}

#[routes]
impl Both {
    #[get("/x")]
    fn x(&self) -> Body {
        Body::text("http")
    }
}

#[patterns]
impl Both {
    #[message_pattern("p.x")]
    async fn y(&self, d: RpcData, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(d)
    }
}

fn main() {}
