//! `#[controller]`, `#[handlers]` and `#[injectable]` are resolved by Rust, so each compiles under
//! any path that reaches it: through a module re-exporting it, through `crate::` and an import at
//! the crate root, and inside `cfg_attr`. The markers they read take `ulo` as their one qualifier
//! whatever path the attribute itself was written through (`tests/ui/marker_qualifier_*.rs`).
//! A renamed dependency and a crate re-exporting `ulo` are `ulo-reexport-renamed` and
//! `ulo-reexport-facade`.

use ulo::{BoxError, Construct, Guard, Handlers};
use ulo_http::{Http, HttpCx};

// What `#[crate::controller]`, `#[crate::handlers]` and `#[crate::injectable]` resolve to.
use ulo::{controller, handlers, injectable};

mod fw {
    pub use ulo::{controller, handlers, injectable};
}

#[fw::controller]
struct Orders {
    #[construct(default)]
    seen: u32,
}

#[fw::handlers]
#[guards(value = Open)]
impl Orders {
    #[ulo_http::get("/orders")]
    #[ulo::meta(1u8)]
    fn list(&self) -> String {
        self.seen.to_string()
    }
}

#[crate::injectable]
struct Ledger {
    #[construct(default)]
    _entries: u32,
}

struct Built;

#[fw::injectable]
impl Built {
    #[construct]
    fn build() -> Self {
        Built
    }
}

struct Till;

#[crate::controller]
#[crate::handlers]
impl Till {
    #[ulo::construct]
    fn open() -> Self {
        Till
    }

    #[ulo_http::get("/till")]
    fn count(&self) -> &'static str {
        "till"
    }
}

#[cfg_attr(all(), fw::controller)]
struct Gated;

#[cfg_attr(all(), crate::handlers)]
impl Gated {
    #[ulo_http::get("/gated")]
    fn gated(&self) -> &'static str {
        "gated"
    }
}

struct Open;

impl Guard<Http> for Open {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

#[test]
fn each_attribute_compiles_through_a_reexport_crate_paths_and_cfg_attr() {
    fn controller<T: Handlers>() {}
    fn constructs<T: Construct>() {}
    controller::<Orders>();
    controller::<Till>();
    controller::<Gated>();
    constructs::<Ledger>();
    constructs::<Built>();
}
