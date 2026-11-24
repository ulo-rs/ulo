//! The `ServiceGenerator` wrapping tonic's: it binds `tonic` inside the client module tonic writes
//! (`scope_tonic`), and after the client it appends one `ulo_grpc::Method` marker per method under
//! the service's module, and the `ulo_grpc::GrpcClient` impl for the client. Each package's file
//! ends with `FILE_DESCRIPTOR_SET` when a descriptor set is written.
//!
//! For the service `users.v1.UserService` with the method `GetUser`, the appended code is:
//!
//! ```text
//! #[allow(clippy::module_inception)]
//! pub mod user_service {
//!     pub struct GetUser;
//!     impl ::ulo_grpc::Method for GetUser {
//!         const PATH: &'static str = "/users.v1.UserService/GetUser";
//!         const SHAPE: ::ulo_grpc::Shape = ::ulo_grpc::Shape::Unary;
//!         type Request = super::GetUserRequest;
//!         type Response = super::User;
//!     }
//! }
//! impl ::ulo_grpc::GrpcClient for user_service_client::UserServiceClient<::ulo_grpc::tonic::transport::Channel> { .. }
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;

use prost_build::{Method, Service, ServiceGenerator};

/// Tonic's generator with the markers appended.
pub(crate) struct Markers {
    pub(crate) tonic: Box<dyn ServiceGenerator>,
    /// Whether tonic writes a client, which then gets the `GrpcClient` impl.
    pub(crate) build_client: bool,
    /// The descriptor set's absolute path, when one is written.
    pub(crate) descriptor: Option<PathBuf>,
}

impl ServiceGenerator for Markers {
    fn generate(&mut self, service: Service, buf: &mut String) {
        let appended = markers(&service, self.build_client);
        let mut client = String::new();
        self.tonic.generate(service, &mut client);
        buf.push_str(&scope_tonic(&client));
        buf.push_str(&appended);
    }

    fn finalize(&mut self, buf: &mut String) {
        self.tonic.finalize(buf);
    }

    fn finalize_package(&mut self, package: &str, buf: &mut String) {
        self.tonic.finalize_package(package, buf);
        if let Some(path) = &self.descriptor {
            let path = path.display().to_string();
            let _ = write!(
                buf,
                "\n/// The encoded file descriptor set `ulo-build` wrote, for `ulo_grpc::Server::file_descriptor_set`.\n\
                 pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!({path:?});\n"
            );
        }
    }
}

/// tonic's client code with `tonic` bound to `ulo_grpc`'s re-export, so the crate running the
/// build step needs no `tonic` of its own.
///
/// tonic-build writes bare `tonic::` paths, with no setting to change them, inside a module of its
/// own, which a `use` outside the module does not reach. The code is parsed, and every inline
/// module in it, at any depth, gains `#[allow(unused_imports)] use ::ulo_grpc::tonic;` as its
/// first item, which binds the name for every path in that module whatever tonic-build writes
/// around it; an item in the module shadows a `tonic` the crate depends on directly, whichever
/// version that is. The file is printed again by `prettyplease`, which tonic-prost-build prints it
/// with, so doc comments and attributes come through as tonic wrote them.
///
/// Code that does not parse as a Rust file comes back as it was. tonic-prost-build prints its
/// client from a parsed `syn` file, and prost-build parses each whole generated file again to
/// format it, so such code stops the build there, with `syn`'s error, whatever this step does.
fn scope_tonic(generated: &str) -> String {
    let Ok(mut file) = syn::parse_file(generated) else {
        return generated.to_owned();
    };
    let binding: syn::Item = syn::parse_quote!(#[allow(unused_imports)] use ::ulo_grpc::tonic;);
    for item in &mut file.items {
        bind_in_modules(item, &binding);
    }
    prettyplease::unparse(&file)
}

/// `binding` as the first item of `item`, when it is an inline module, and of every inline module
/// inside it. The file's own top level is left alone: it is `include!`d into the user's module,
/// where a `tonic` of the user's own may already be in scope.
fn bind_in_modules(item: &mut syn::Item, binding: &syn::Item) {
    if let syn::Item::Mod(syn::ItemMod { content: Some((_, items)), .. }) = item {
        for inner in items.iter_mut() {
            bind_in_modules(inner, binding);
        }
        items.insert(0, binding.clone());
    }
}

/// The marker module of `service`, and the client's `GrpcClient` impl when tonic writes a client.
///
/// The module is named as tonic names its client module, `user_service` beside
/// `user_service_client`, by tonic-build's own snake-casing, so the two agree character for
/// character.
fn markers(service: &Service, build_client: bool) -> String {
    let module = snake_case(&service.name);
    let qualified = if service.package.is_empty() {
        service.proto_name.clone()
    } else {
        format!("{}.{}", service.package, service.proto_name)
    };
    let mut out = String::new();
    let _ = writeln!(out, "\n/// The method markers of `{qualified}`, which `#[ulo_grpc::method(..)]` binds handlers to.");
    // A wrapper module named as the service, `mod users { include_proto!("users.v1"); }` for the
    // service `Users`, holds `users::users`, which clippy's `module_inception` would report in
    // the user's crate on a module the user cannot rename.
    let _ = writeln!(out, "#[allow(clippy::module_inception)]");
    let _ = writeln!(out, "pub mod {module} {{");
    for method in &service.methods {
        let _ = write!(out, "{}", marker(method, &qualified));
    }
    let _ = writeln!(out, "}}");
    if build_client {
        let client = format!("{module}_client::{}Client<::ulo_grpc::tonic::transport::Channel>", service.name);
        let _ = writeln!(out, "impl ::ulo_grpc::GrpcClient for {client} {{");
        let _ = writeln!(out, "    fn from_channel(channel: ::ulo_grpc::tonic::transport::Channel) -> Self {{");
        let _ = writeln!(out, "        Self::new(channel)");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out, "}}");
    }
    out
}

fn marker(method: &Method, service: &str) -> String {
    let name = upper_camel(&method.proto_name);
    let path = format!("/{service}/{}", method.proto_name);
    let shape = match (method.client_streaming, method.server_streaming) {
        (false, false) => "Unary",
        (false, true) => "ServerStreaming",
        (true, false) => "ClientStreaming",
        (true, true) => "Bidi",
    };
    let request = type_path(&method.input_type);
    let response = type_path(&method.output_type);
    format!(
        "    /// `{path}`.\n\
         \x20   pub struct {name};\n\
         \x20   impl ::ulo_grpc::Method for {name} {{\n\
         \x20       const PATH: &'static str = {path:?};\n\
         \x20       const SHAPE: ::ulo_grpc::Shape = ::ulo_grpc::Shape::{shape};\n\
         \x20       type Request = {request};\n\
         \x20       type Response = {response};\n\
         \x20   }}\n"
    )
}

/// A message's Rust path as written inside the marker module, one level below the file prost
/// resolved it against: a path prost made relative gains `super::`, while an extern path, `()` for
/// `google.protobuf.Empty` and `::prost_types::..` for the other well-known types, stays as it is.
fn type_path(rust: &str) -> String {
    if rust.starts_with("::") || rust.starts_with("crate::") || rust.starts_with('(') {
        rust.to_owned()
    } else {
        format!("super::{rust}")
    }
}

/// tonic-build's `naive_snake_case`: lowercase each character and put `_` before each uppercase
/// one but the first, so `ABCService` is `a_b_c_service`.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c.to_ascii_lowercase());
        if chars.peek().is_some_and(|next| next.is_uppercase()) {
            out.push('_');
        }
    }
    out
}

/// `GetUser` stays `GetUser`; `get_user` becomes `GetUser`.
fn upper_camel(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::scope_tonic;

    const BINDING: &str = "#[allow(unused_imports)]\n    use ::ulo_grpc::tonic;";

    #[test]
    fn a_client_module_without_tonics_glob_import_still_gains_the_binding() {
        let generated = "pub mod greeter_client {\n    pub struct GreeterClient<T> {\n        inner: tonic::client::Grpc<T>,\n    }\n}\n";

        let scoped = scope_tonic(generated);

        assert!(scoped.contains(&format!("pub mod greeter_client {{\n    {BINDING}\n")), "{scoped}");
    }

    #[test]
    fn doc_comments_and_inner_attributes_come_through() {
        let generated = "/// Generated client implementations.\npub mod greeter_client {\n    #![allow(dead_code)]\n    use tonic::codegen::*;\n    /// Says hello.\n    #[derive(Debug, Clone)]\n    pub struct GreeterClient;\n}\n";

        let scoped = scope_tonic(generated);

        for kept in ["/// Generated client implementations.", "#![allow(dead_code)]", "/// Says hello.", "#[derive(Debug, Clone)]", "use tonic::codegen::*;"] {
            assert!(scoped.contains(kept), "`{kept}` is missing from:\n{scoped}");
        }
        assert!(scoped.find(BINDING) < scoped.find("use tonic::codegen::*;"), "{scoped}");
    }

    #[test]
    fn a_module_nested_in_the_client_module_gains_its_own_binding() {
        let generated = "pub mod outer {\n    pub mod inner {\n        pub type Channel = tonic::transport::Channel;\n    }\n}\n";

        let scoped = scope_tonic(generated);

        assert_eq!(scoped.matches("use ::ulo_grpc::tonic;").count(), 2, "{scoped}");
        assert!(scoped.contains("pub mod inner {\n        #[allow(unused_imports)]\n        use ::ulo_grpc::tonic;"), "{scoped}");
    }

    #[test]
    fn the_top_level_and_an_outlined_module_are_left_alone() {
        let generated = "pub mod declared;\npub struct Top;\n";

        assert_eq!(scope_tonic(generated), generated);
    }

    #[test]
    fn text_that_is_not_rust_comes_back_as_it_was() {
        let generated = "pub mod greeter_client { this is not rust";

        assert_eq!(scope_tonic(generated), generated);
    }
}
