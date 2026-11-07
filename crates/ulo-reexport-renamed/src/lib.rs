//! The attributes `#[handlers]`, `#[controller]` and `#[injectable]` read and remove compile with
//! `ulo` renamed in the manifest, `framework = { package = "ulo", .. }`.
//!
//! Beside the item, each macro writes a hidden import of every such attribute, so an editor
//! resolves it. That import, like the rest of the generated code, names `::ulo`, which the
//! `extern crate` alias below provides. A marker is written bare or through that alias,
//! `#[ulo::guards]`; `#[framework::guards]` is refused at the attribute, since `ulo` is the one
//! qualifier the generated code names.
//!
//! The crate also re-exports `framework` whole, so `ulo-reexport-facade` reaches the macros
//! through it without listing `ulo` at all.

extern crate framework as ulo;

pub use framework::*;

#[cfg(test)]
mod markers {
    use framework::{BoxError, Construct, Guard, Handlers, controller, handlers, injectable};
    use ulo_http::{Http, HttpCx};

    struct Deny;

    impl Guard<Http> for Deny {
        async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
            Ok(false)
        }
    }

    #[controller]
    struct Orders {
        #[controller(default)]
        seen: u32,
    }

    #[handlers]
    #[guards(value = Deny)]
    #[meta(1u8)]
    impl Orders {
        #[ulo_http::get("/orders")]
        #[ulo::guards(value = Deny)]
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
        fn controller<T: Handlers>() {}
        fn constructs<T: Construct>() {}
        controller::<Orders>();
        constructs::<Orders>();
        constructs::<Built>();
    }
}
