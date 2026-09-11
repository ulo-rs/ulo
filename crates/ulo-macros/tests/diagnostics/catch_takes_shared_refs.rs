// A `#[catch(T)]` handler receives the error and the context by shared
// reference. `&mut` is refused rather than silently accepted.
use ulo::context::HttpContext;
use ulo::http_helpers::HttpResponse;

#[derive(Debug, ulo::Error)]
#[error_kind(BadRequest)]
pub struct MyError;

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("my error")
    }
}

#[ulo::catch(MyError)]
async fn handle(_e: &mut MyError, _ctx: &HttpContext) -> HttpResponse {
    HttpResponse::default()
}

fn main() {}
