// ADR-0041: every RPC handler parameter is a `FromContext<RpcContext>`. The
// bare-DTO form is gone, and a plain type fails at the parameter with the note
// naming what to write.
use ulo::rpc::RpcContext;
use ulo::rpc::{RpcData, RpcError};
use ulo::{controller, patterns};
#[controller]
pub struct Orders {}

#[patterns]
impl Orders {
    #[message_pattern("orders.get")]
    async fn get(&self, _not_an_extractor: String, _c: &RpcContext) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!({})))
    }
}

fn main() {}
