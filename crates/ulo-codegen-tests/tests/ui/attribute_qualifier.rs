// `#[controller]`, `#[handlers]` and `#[injectable]` invoked through a renamed dependency rather
// than bare or as `ulo::<name>`: Rust resolves each, and the generated code names `::ulo`, which a
// renamed dependency aliases with `extern crate .. as ulo`. Decision 27 (F595).
// Refused at: each attribute, with "write the attribute bare, or as `ulo::<name>`".
extern crate ulo as framework;

#[framework::injectable]
struct Ledger;

#[framework::controller]
struct Till;

struct Register;

#[framework::handlers]
impl Register {
    #[ulo_http::get("/register")]
    fn get(&self) -> &'static str {
        "register"
    }
}

fn main() {}
