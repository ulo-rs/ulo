use std::time::Duration;

use ulo::{controller, handlers};
use ulo_http::Timeout;

#[controller]
struct Chat;

#[handlers]
#[ulo_ws::gateway(path = "/chat")]
impl Chat {
    #[ulo_ws::message("chat.send")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    async fn send(&self) {}
}

fn main() {}
