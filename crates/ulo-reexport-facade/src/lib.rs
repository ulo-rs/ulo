//! The attributes `#[handlers]`, `#[controller]` and `#[injectable]` read and remove compile in a
//! crate that reaches `ulo` only through a crate re-exporting it, `ulo-reexport-renamed`, and lists
//! no `ulo` in its manifest.
//!
//! The generated code, the hidden import for an editor included, names `::ulo`, which the
//! `extern crate` alias below provides.

extern crate ulo_reexport_renamed as ulo;

#[cfg(test)]
mod markers {
    use ulo::{BoxError, Construct, Controller, Guard, controller, handlers, injectable};
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
    fn the_markers_compile_with_no_ulo_in_the_manifest() {
        fn controller<T: Controller>() {}
        fn constructs<T: Construct>() {}
        controller::<Orders>();
        constructs::<Orders>();
        constructs::<Built>();
    }
}
