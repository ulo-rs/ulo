//! The attributes `#[handlers]`, `#[controller]` and `#[injectable]` read and remove compile in a
//! crate that reaches `ulo` only through a crate re-exporting it, `ulo-reexport-renamed`, and lists
//! no `ulo` in its manifest.
//!
//! The generated code, the hidden import for an editor included, names `::ulo`, which the
//! `extern crate` alias below provides. The attributes also compile under the re-exporting
//! crate's own name, `#[ulo_reexport_renamed::controller]`, and as its `service!` macro writes
//! them.

extern crate ulo_reexport_renamed as ulo;

#[cfg(test)]
mod markers {
    use ulo::{BoxError, Construct, Guard, Handlers, controller, handlers, injectable};
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
        fn controller<T: Handlers>() {}
        fn constructs<T: Construct>() {}
        controller::<Orders>();
        constructs::<Orders>();
        constructs::<Built>();
    }

    #[ulo_reexport_renamed::controller]
    struct Facade {
        #[controller(default)]
        seen: u32,
    }

    #[ulo_reexport_renamed::handlers]
    #[guards(value = Deny)]
    impl Facade {
        #[ulo_http::get("/facade")]
        fn get(&self) -> String {
            self.seen.to_string()
        }
    }

    struct Ledger;

    #[ulo_reexport_renamed::injectable]
    impl Ledger {
        #[construct]
        fn open() -> Self {
            Ledger
        }
    }

    ulo_reexport_renamed::service! {
        struct Served;

        impl Served {
            #[ulo_http::get("/served")]
            #[ulo::guards(value = Deny)]
            fn get(&self) -> &'static str {
                "served"
            }
        }
    }

    #[test]
    fn the_attributes_compile_under_the_reexporting_crates_name_and_its_macro() {
        fn controller<T: Handlers>() {}
        fn constructs<T: Construct>() {}
        controller::<Facade>();
        controller::<Served>();
        constructs::<Ledger>();
    }
}
