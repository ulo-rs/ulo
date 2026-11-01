//! `#[derive(Classify)]` and `#[derive(Validate)]` work through `ulo-rpc` alone.
//!
//! The derives name the first of `ulo-transport`, `ulo-rpc`, `ulo-http`, `ulo-ws` and `ulo-grpc`
//! the deriving crate's manifest lists. This crate lists `ulo-rpc` and nothing else, so the
//! generated code reaches `ulo_rpc::__derive` and fails to compile if that re-export goes.
//!
//! The manifest lists nothing else, which this pins: with `ulo-transport` beside `ulo-rpc` the
//! derives would reach `ulo_transport` and the re-export would go unchecked.
//!
//! ```compile_fail,E0463
//! extern crate ulo_transport;
//! ```

#[cfg(test)]
mod derives {
    use ulo_rpc::{Classify, ErrorKind, Validate};

    #[derive(Debug, Classify)]
    enum Failure {
        #[classify(not_found)]
        Missing,
    }

    impl std::fmt::Display for Failure {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("missing")
        }
    }

    impl std::error::Error for Failure {}

    #[derive(Validate)]
    struct Form {
        #[validate(length(min = 1))]
        name: String,
    }

    #[test]
    fn classify_derives_through_the_one_crate() {
        assert_eq!(Failure::Missing.classify(), ErrorKind::NotFound);
    }

    #[test]
    fn validate_derives_through_the_one_crate() {
        assert!(Form { name: String::new() }.validate().is_err());
        assert!(Form { name: "a".into() }.validate().is_ok());
    }
}
