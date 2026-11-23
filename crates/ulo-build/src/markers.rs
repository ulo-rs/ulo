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
        if self.build_client {
            client = scope_tonic(&client);
        }
        buf.push_str(&client);
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

/// tonic's client module with `tonic` bound to `ulo_grpc`'s re-export, so the crate running the
/// build step needs no `tonic` of its own.
///
/// tonic-build writes bare `tonic::` paths, with no setting to change them, inside a module of its
/// own, which a `use` outside the module does not reach. One `use ::ulo_grpc::tonic;` above the
/// module's first import binds the name for every path in the module, and an item in the module
/// shadows a `tonic` the crate depends on directly, whichever version that is.
///
/// # Panics
///
/// When the client has no `use tonic::codegen::*;` line, which a tonic-build writing its client
/// differently would cause: the build script stops there, naming the line and the two ways past
/// it, rather than leaving every `tonic::` path in the client unresolved.
fn scope_tonic(client: &str) -> String {
    const ANCHOR: &str = "use tonic::codegen::*;";
    let Some((head, tail)) = client.split_once(ANCHOR) else {
        panic!(
            "ulo-build: the client module tonic-build wrote has no `{ANCHOR}` line, above which ulo-build binds `tonic` to \
             `ulo_grpc::tonic`; this tonic-build release writes its clients differently from the ones ulo-build supports. \
             Hold tonic-build at an earlier release with `cargo update -p tonic-build --precise <version>`, or generate no \
             clients with `ulo_build::configure().build_client(false)`"
        );
    };
    format!("{head}use ::ulo_grpc::tonic;\n    {ANCHOR}{tail}")
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
