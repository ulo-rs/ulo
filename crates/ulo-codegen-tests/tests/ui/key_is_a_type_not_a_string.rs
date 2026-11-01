// Capability 9: "Keys are compiler-checked, no string keys" (DESIGN §3.1), a lookup by string.
// Capability 46.
// Refused at: the string literal, where `get` takes a type argument and no value.
use ulo::AppHandle;

async fn read(app: &AppHandle) {
    let _ = app.get::<u32>("primary").await;
}

fn main() {}
