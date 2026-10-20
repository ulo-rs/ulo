// Capability 9: "Keys are compiler-checked, no string keys" (DESIGN §3.1), a `Key` built from a
// string. Capability 46.
// Refused at: the call, since `Key` has `Key::of::<T, Q>()` and no `From<&str>`.
use ulo::Key;

fn main() {
    let _: Key = Key::from("primary");
}
