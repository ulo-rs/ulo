// An `#[sse]` handler answers with a stream, a stream of `Result`, or a `Result`
// of either. The conversion refuses anything else, and the message names all
// three shapes.
use ulo::http::HttpResponse;
use ulo::{controller, routes, sse};

#[controller("/p")]
pub struct C {}

#[routes]
impl C {
    #[sse("/x")]
    async fn x(&self) -> HttpResponse {
        HttpResponse::ok().build()
    }
}

fn main() {}
