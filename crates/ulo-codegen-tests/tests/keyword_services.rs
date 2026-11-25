//! A service named after a Rust keyword compiles: its marker module is the raw identifier, or the
//! name with a trailing `_` for `super`, and its client module is tonic's own `match_client`.

use ulo_codegen_tests::keywords::{r#match, r#type, match_client::MatchClient, super_};
use ulo_grpc::Method;

#[test]
fn a_service_named_after_a_keyword_has_a_marker_module_under_the_raw_identifier() {
    assert_eq!(<r#match::Go as Method>::PATH, "/codegen.keywords.v1.Match/Go");
    assert_eq!(<r#type::Go as Method>::PATH, "/codegen.keywords.v1.Type/Go");
    assert_eq!(<super_::Go as Method>::PATH, "/codegen.keywords.v1.Super/Go");
    let _ = std::any::type_name::<MatchClient<ulo_grpc::tonic::transport::Channel>>();
}
