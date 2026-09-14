// The scope was never one HTTP request, and `scope = "request"` is refused rather than aliased,
// so the diagnostic is the only thing telling a reader what to write instead.
use ulo::injectable;

#[injectable(scope = "request")]
pub struct CallerIdentity {}

fn main() {}
