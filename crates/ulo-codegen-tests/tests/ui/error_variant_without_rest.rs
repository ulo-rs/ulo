// §10.2: every struct variant of `StartupError` and `ConnectError` is `#[non_exhaustive]`, so a
// pattern outside the core names its fields with `..`, and a field added later breaks no caller.
// Refused at: each pattern written without `..`.
use ulo::{ConnectError, StartupError};

fn hook(error: &ConnectError) -> bool {
    matches!(error, ConnectError::Hook { hook: _, key: _, reason: _ })
}

fn bind(error: &StartupError) -> bool {
    matches!(error, StartupError::Bind { transport: _, source: _ })
}

fn main() {
    let _ = (hook, bind);
}
