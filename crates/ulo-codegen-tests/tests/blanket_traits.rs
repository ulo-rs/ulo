//! The code the attributes generate compiles in a module where a user trait implemented for every
//! type has a method of the same name as one the generated code calls by method syntax. Two sets
//! of such traits, one whose methods take `&self` and one whose methods take `self`, each over
//! the same application: `#[injectable]` on a struct and on a constructor with parameters, hooks
//! probed by `#[injectable]`, a `#[module]` with every kind of providers entry, `#[routes]` with
//! impl- and method-level enhancers and metadata on an HTTP, a WebSocket, an RPC and a gRPC
//! controller, a gateway with connect guards and connection hooks, and `#[derive(Validate)]`.
//!
//! The probes the generated code ranks by autoref are trait methods, which the user's blanket
//! trait meets at the same autoref step: ambiguous (E0034) where the probe's arm applies, and the
//! user's method where it does not. A probe method named after the hook, the check or the rule it
//! probes, `field` or `check` or `length`, fails this file to compile. So does an inherent method
//! taking `&mut self` that the generated code calls by method syntax on an owned value, which a
//! `&self` method of the user's reaches one step earlier. Capability: the generated code is
//! hygienic in the user's scope.

macro_rules! names {
    ($receiver:ident: $($name:ident)*) => {
        $(
            // Named as the generated code's calls are, `into_reply(&self)` and `is_empty(self)` among them.
            #[allow(non_camel_case_types, dead_code, clippy::wrong_self_convention)]
            pub trait $name {
                names!(@method $receiver $name);
            }
            impl<T: ?Sized> $name for T {}
        )*
    };
    (@method by_ref $name:ident) => {
        fn $name(&self) {}
    };
    (@method by_value $name:ident) => {
        fn $name(self)
        where
            Self: Sized,
        {
        }
    };
}

/// Every method name the generated code calls by method syntax, probes and builders alike.
macro_rules! blanket {
    ($receiver:ident) => {
        names!($receiver:
            field param register_init register_bootstrap register_destroy register_before_shutdown
            register_shutdown bind bind_as contribute_with check into_reply kind streams event_reply
            read on_connect on_disconnect after_init length body matches answer checked push is_empty
            controller method meta route shape dependencies handler once payload path_check host_read
            provide value try_value contribute also_as qualified export_qualified reexport_qualified export import global secret guard guard_arc
            guard_value guard_with interceptor interceptor_arc error_handler error_handler_arc
            validate codec port subprotocols refuse overflow message_limit construct
        );
    };
}

/// The application, written once per set of traits.
macro_rules! application {
    () => {
        use std::convert::Infallible;
        use std::time::Duration;

        use ulo::{BoxError, Dep, Guard, OnModuleInit, injectable, module, routes};
        use ulo_codegen_tests::pb::{self, AddRequest, Sum};
        use ulo_grpc::Message as Grpc;
        use ulo_http::{Http, HttpCx, Path, Timeout};
        use ulo_rpc::Payload;
        use ulo_transport::Validate;
        use ulo_ws::{ConnectCx, ConnectRefused, OnConnect, WsConnect};

        #[injectable]
        pub struct Store;

        /// Implements `OnModuleInit`, so the hooks probe takes its registering arm.
        #[injectable]
        pub struct Holder {
            _store: Dep<Store>,
        }

        impl OnModuleInit for Holder {
            async fn on_module_init(&self) -> Result<(), BoxError> {
                Ok(())
            }
        }

        pub struct Built {
            _holder: Dep<Holder>,
        }

        #[injectable]
        impl Built {
            fn new(holder: Dep<Holder>, _store: Dep<Store>) -> Self {
                Built { _holder: holder }
            }
        }

        pub trait Plugin: Send + Sync {}

        #[injectable]
        pub struct Metrics;

        impl Plugin for Metrics {}

        pub struct Marker;

        pub trait Sink: Send + Sync {}

        pub struct Drain;

        impl Sink for Drain {}

        #[derive(Validate)]
        pub struct Form {
            #[validate(length(min = 3))]
            pub name: String,
            #[validate(length(max = 2))]
            pub tags: Vec<String>,
            #[validate(range(min = 18))]
            pub age: u32,
            #[validate(email)]
            pub email: String,
        }

        pub struct Admit;

        impl Guard<Http> for Admit {
            async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
                Ok(true)
            }
        }

        impl Guard<WsConnect> for Admit {
            async fn can_activate(&self, _cx: &ConnectCx) -> Result<bool, BoxError> {
                Ok(true)
            }
        }

        pub struct Tag(&'static str);

        #[injectable]
        pub struct Web;

        #[routes]
        #[guards(value = Admit)]
        #[meta(Tag("controller"))]
        impl Web {
            #[ulo_http::get("/x/{id}")]
            #[guards(Admit, value = Admit)]
            #[meta(Tag("method"), Timeout::after(Duration::from_secs(5)))]
            async fn get(&self, Path(id): Path<u32>) -> String {
                id.to_string()
            }
        }

        #[injectable]
        pub struct Chat;

        #[routes]
        #[ulo_ws::gateway(path = "/ws", connect_guards(Admit, value = Admit))]
        impl Chat {
            #[ulo_ws::message("m")]
            async fn m(&self) -> String {
                String::new()
            }
        }

        impl OnConnect for Chat {
            async fn on_connect(&self, _cx: &ConnectCx) -> Result<(), ConnectRefused> {
                Ok(())
            }
        }

        #[injectable]
        pub struct Bus;

        #[routes]
        impl Bus {
            #[ulo_rpc::message("bus.m")]
            async fn m(&self, n: Payload<u32>) -> Result<u32, Infallible> {
                Ok(n.0)
            }

            #[ulo_rpc::event("bus.e")]
            async fn e(&self, _n: Payload<u32>) {}
        }

        #[injectable]
        pub struct Counter;

        #[routes]
        impl Counter {
            #[ulo_grpc::method(pb::counter::Add)]
            async fn add(&self, req: Grpc<AddRequest>) -> Sum {
                Sum { sum: req.0.a + req.0.b }
            }
        }

        #[module(
            providers = [
                Store,
                Holder,
                Built,
                Metrics as dyn Plugin,
                { 7_u8 },
                with = |s: Dep<Store>| { let _ = s; 7_u16 },
                dyn Sink: with = || Drain,
                into dyn Plugin: [with = || Metrics, value = Metrics],
                Holder @ Marker,
                Metrics as dyn Plugin @ Marker,
                { 3_u32 } @ Marker,
            ],
            controllers = [Web, Chat, Bus, Counter],
            exports = [Store, u32 @ Marker],
        )]
        pub struct Root;

        pub fn validate(form: &Form) -> bool {
            <Form as Validate>::validate(form).is_ok()
        }
    };
}

// The application is compiled for its generated code; nothing builds it.
#[allow(dead_code)]
mod by_ref {
    blanket!(by_ref);
    application!();
}

#[allow(dead_code)]
mod by_value {
    blanket!(by_value);
    application!();
}

/// The validation the derive wrote still answers by its rules with the user's traits in scope.
#[test]
fn derived_validation_reads_its_rules_beside_user_traits() {
    let ok = by_ref::Form {
        name: "abc".to_owned(),
        tags: vec![],
        age: 18,
        email: "a@b.io".to_owned(),
    };
    assert!(by_ref::validate(&ok));
    let short = by_value::Form {
        name: "ab".to_owned(),
        tags: vec![],
        age: 18,
        email: "a@b.io".to_owned(),
    };
    assert!(!by_value::validate(&short));
}
