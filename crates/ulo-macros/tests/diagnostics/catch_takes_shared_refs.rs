// A `#[catch(T)]` handler receives the error and the context by shared
// reference. `&mut` is refused rather than silently accepted.
//
// Paths are written in full and `MyError` implements `std::error::Error`, so
// the recorded output is this diagnostic and nothing else. A fixture that also
// trips an unrelated bound buries the message the case exists to pin.

#[derive(Debug, ulo::Error)]
#[error_kind(BadRequest)]
pub struct MyError;

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("my error")
    }
}

impl std::error::Error for MyError {}

#[ulo::catch(MyError)]
async fn handle(
    _e: &mut MyError,
    _ctx: &ulo::context::HttpContext,
) -> ulo::HttpResponse {
    ulo::HttpResponse::default()
}

fn main() {}
