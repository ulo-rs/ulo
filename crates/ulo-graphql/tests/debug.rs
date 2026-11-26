use serde_json::json;
use ulo_graphql::GqlRequest;

fn login() -> GqlRequest {
    let mut request = GqlRequest::new("mutation Login($user: String!, $password: String!) { login(user: $user, password: $password) }");
    request.operation_name = Some("Login".into());
    request.variables = json!({"user": "ada", "password": "hunter2-planted"}).as_object().cloned();
    request.extensions = json!({"authorization": "Bearer planted-token", "persistedQuery": {"sha256Hash": "planted-hash"}}).as_object().cloned();
    request
}

#[test]
fn debug_prints_names_and_no_caller_values() {
    for text in [format!("{:?}", login()), format!("{:#?}", login())] {
        for planted in ["hunter2-planted", "\"ada\"", "Bearer planted-token", "planted-hash"] {
            assert!(!text.contains(planted), "{planted} printed in {text}");
        }
        for shown in ["mutation Login($user", "authorization", "persistedQuery"] {
            assert!(text.contains(shown), "{shown} missing from {text}");
        }
    }
    let text = format!("{:?}", login());
    assert!(text.contains(r#"operation_name: Some("Login")"#), "{text}");
    let names = text.split("variable_names: Some([").nth(1).and_then(|rest| rest.split("])").next()).expect("variable_names lists its keys");
    assert!(names.contains(r#""password""#) && names.contains(r#""user""#), "{text}");
}

#[test]
fn absent_maps_print_as_none() {
    let text = format!("{:?}", GqlRequest::new("{ ping }"));
    assert!(text.contains("variable_names: None") && text.contains("extension_names: None"), "{text}");
}
