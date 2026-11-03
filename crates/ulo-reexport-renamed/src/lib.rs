//! The attributes `#[routes]` and `#[injectable]` read and remove compile with `ulo` renamed in
//! the manifest, `framework = { package = "ulo", .. }`.
//!
//! Beside the item, each macro writes a hidden import of every such attribute, so an editor
//! resolves it. That import, like the rest of the generated code, names `::ulo`, which the
//! `extern crate` alias below provides.
//!
//! The crate also re-exports `framework` whole, so `ulo-reexport-facade` reaches the macros
//! through it without listing `ulo` at all.

extern crate framework as ulo;

pub use framework::*;

#[cfg(test)]
mod markers {
    use framework::{BoxError, Construct, Controller, Guard, injectable, routes};
    use ulo_http::{Http, HttpCx};

    struct Deny;

    impl Guard<Http> for Deny {
        async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
            Ok(false)
        }
    }

    #[injectable]
    struct Orders {
        #[injectable(default)]
        seen: u32,
    }

    #[routes]
    #[guards(value = Deny)]
    #[meta(1u8)]
    impl Orders {
        #[ulo_http::get("/orders")]
        #[framework::guards(value = Deny)]
        fn list(&self) -> String {
            self.seen.to_string()
        }
    }

    struct Built;

    #[injectable]
    impl Built {
        #[construct]
        fn build() -> Self {
            Built
        }
    }

    #[test]
    fn the_markers_compile_under_the_renamed_crate() {
        fn controller<T: Controller>() {}
        fn constructs<T: Construct>() {}
        controller::<Orders>();
        constructs::<Orders>();
        constructs::<Built>();
    }
}
