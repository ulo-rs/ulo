//! What each method of a tonic-generated service carries, written beside the
//! trait so `#[grpc_methods]` can name it without reading a handler.
//!
//! tonic-build turns a `.proto` service into a trait whose methods are declared
//! with their request types: `tonic::Request<GreetRequest>`, or
//! `tonic::Request<tonic::Streaming<GreetRequest>>` when the caller streams.
//! The impl `#[grpc_methods]` writes has to repeat those types, and a macro
//! cannot resolve a name to find them. This crate reads them off the trait
//! tonic wrote and appends a companion module — one marker type per method,
//! implementing `ulo_grpc::MethodShape` — that the macro projects through:
//!
//! ```ignore
//! // build.rs
//! tonic_prost_build::compile_protos("proto/orders.proto")?;
//! ulo_build::shapes("ulo_examples.orders")?;
//! ```
//!
//! [`shapes`] takes the string `tonic::include_proto!` takes and rewrites the
//! file that macro includes in place, so nothing else in the crate changes.
//! A trait tonic did not write — hand-written, or generated somewhere the
//! build script cannot reach — goes through [`shapes_in_file`] instead.
//!
//! The companion sits beside the `*_server` module at the same depth, so the
//! `super::` paths tonic wrote resolve to the same items from inside it.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Item, ItemTrait, TraitItem, Type};

/// Marks where this crate's output begins in a rewritten file, so a second
/// run replaces its own section rather than appending another.
const SENTINEL: &str = "// ==== ulo-build: what each method carries, for #[grpc_methods] ====";

/// Write the shapes for the services in `$OUT_DIR/{package}.rs`.
///
/// `package` is the string handed to `tonic::include_proto!` — the proto
/// package for a service tonic-prost-build compiled, or `package.Service` for
/// one built through `tonic_build::manual`. Call it after the tonic step that
/// writes the file.
pub fn shapes(package: &str) -> Result<(), Error> {
    let out_dir = std::env::var_os("OUT_DIR").ok_or(Error::NoOutDir)?;
    let path = PathBuf::from(out_dir).join(format!("{package}.rs"));
    shapes_in_file(&path)
}

/// Write the shapes for the services in a file, whatever wrote it.
pub fn shapes_in_file(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    let source = std::fs::read_to_string(path).map_err(|e| Error::Io(e, path.to_path_buf()))?;
    let rewritten = with_companion(&source)?;
    std::fs::write(path, rewritten).map_err(|e| Error::Io(e, path.to_path_buf()))
}

/// The source with its companion modules appended, replacing any earlier run's.
fn with_companion(source: &str) -> Result<String, Error> {
    let kept = source.find(SENTINEL).map_or(source, |at| &source[..at]);
    let companion = companion(kept)?;
    if companion.is_empty() {
        return Err(Error::NoService);
    }
    Ok(format!("{}\n{SENTINEL}\n{companion}", kept.trim_end()))
}

/// The companion module for every service trait in `source`, formatted; empty
/// when the source declares none.
pub fn companion(source: &str) -> Result<String, Error> {
    let file = syn::parse_file(source).map_err(Error::Parse)?;
    let mut modules = TokenStream::new();

    for item in &file.items {
        match item {
            Item::Trait(service) => modules.extend(companion_for(service)?),
            Item::Mod(module) => {
                for inner in module.content.iter().flat_map(|(_, items)| items) {
                    if let Item::Trait(service) = inner {
                        modules.extend(companion_for(service)?);
                    }
                }
            }
            _ => {}
        }
    }

    if modules.is_empty() {
        return Ok(String::new());
    }
    let file: syn::File = syn::parse2(modules).map_err(Error::Parse)?;
    Ok(prettyplease::unparse(&file))
}

/// The module for one trait, or nothing for a trait no method of which takes
/// a `Request<_>` — tonic's client module declares none.
fn companion_for(service: &ItemTrait) -> Result<TokenStream, Error> {
    let mut markers = TokenStream::new();

    for item in &service.items {
        let TraitItem::Fn(method) = item else {
            continue;
        };
        let Some(arg) = request_argument(&method.sig) else {
            continue;
        };
        let marker = format_ident!("{}", to_upper_camel(&method.sig.ident.to_string()));
        let install = if is_streaming(&arg) {
            quote! { ::ulo_grpc::shape::stream(request, ctx) }
        } else {
            quote! { ::ulo_grpc::shape::message(request, ctx) }
        };
        markers.extend(quote! {
            pub struct #marker;
            impl ::ulo_grpc::MethodShape for #marker {
                type Arg = #arg;
                fn install(
                    request: ::tonic::Request<Self::Arg>,
                    ctx: &::ulo::context::GrpcContext,
                ) {
                    #install
                }
            }
        });
    }

    if markers.is_empty() {
        return Ok(TokenStream::new());
    }

    let trait_name = service.ident.to_string();
    let module = format_ident!("{}_ulo", to_snake(&trait_name));
    let doc = format!(
        " What each method of `{trait_name}` carries, read off the trait tonic \
         generated. Written by ulo-build; `#[grpc_methods]` projects through these."
    );
    Ok(quote! {
        #[doc = #doc]
        pub mod #module {
            #markers
        }
    })
}

/// The `T` of a method's `request: Request<T>`, as written.
fn request_argument(sig: &syn::Signature) -> Option<Type> {
    let syn::FnArg::Typed(param) = sig.inputs.iter().nth(1)? else {
        return None;
    };
    let Type::Path(path) = param.ty.as_ref() else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != "Request" {
        return None;
    }
    first_type_argument(last)
}

/// Whether the wire carries `tonic::Streaming<_>` rather than one message.
fn is_streaming(arg: &Type) -> bool {
    let Type::Path(path) = arg else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "Streaming")
}

fn first_type_argument(segment: &syn::PathSegment) -> Option<Type> {
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first() {
        Some(syn::GenericArgument::Type(inner)) => Some(inner.clone()),
        _ => None,
    }
}

/// tonic-build's own snake-casing, so `Greeter` names `greeter_ulo` beside
/// the `greeter_server` it wrote. `#[grpc_methods]` derives the same name
/// from the trait path, so the two must agree character for character.
fn to_snake(name: &str) -> String {
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

/// `greet_all` names the marker `GreetAll`; a raw identifier drops its `r#`.
fn to_upper_camel(ident: &str) -> String {
    ident
        .trim_start_matches("r#")
        .split('_')
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

#[derive(Debug)]
pub enum Error {
    /// Not running under cargo: `OUT_DIR` is unset.
    NoOutDir,
    Io(io::Error, PathBuf),
    /// The file is not Rust tonic wrote.
    Parse(syn::Error),
    /// The file declares no trait whose methods take a `Request<_>`.
    NoService,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoOutDir => write!(
                f,
                "ulo_build::shapes reads $OUT_DIR, which cargo sets only for a build script"
            ),
            Error::Io(e, path) => write!(f, "{}: {e}", path.display()),
            Error::Parse(e) => write!(f, "could not parse the generated code: {e}"),
            Error::NoService => write!(
                f,
                "no service trait found — run this after the tonic step that writes the file, \
                 with the string you hand to `tonic::include_proto!`"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e, _) => Some(e),
            Error::Parse(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from tonic-build's output for the test suite's `Greeter`: one
    /// method per call shape, plus the client module that declares no trait.
    const GENERATED: &str = r#"
        pub struct GreetRequest {}
        pub mod greeter_server {
            pub trait Greeter: Send + Sync + 'static {
                async fn greet(
                    &self,
                    request: tonic::Request<super::GreetRequest>,
                ) -> Result<tonic::Response<super::GreetReply>, tonic::Status>;
                type GreetManyStream: Stream<Item = Result<super::GreetReply, tonic::Status>> + Send + 'static;
                async fn greet_many(
                    &self,
                    request: tonic::Request<super::GreetRequest>,
                ) -> Result<tonic::Response<Self::GreetManyStream>, tonic::Status>;
                async fn greet_all(
                    &self,
                    request: tonic::Request<tonic::Streaming<super::GreetRequest>>,
                ) -> Result<tonic::Response<super::GreetReply>, tonic::Status>;
                type ConverseStream: Stream<Item = Result<super::GreetReply, tonic::Status>> + Send + 'static;
                async fn converse(
                    &self,
                    request: tonic::Request<tonic::Streaming<super::GreetRequest>>,
                ) -> Result<tonic::Response<Self::ConverseStream>, tonic::Status>;
            }
            pub struct GreeterServer<T> { inner: T }
        }
        pub mod greeter_client {
            pub struct GreeterClient<T> { inner: T }
        }
    "#;

    #[test]
    fn every_call_shape_gets_a_marker_naming_what_the_wire_carries() {
        let out = companion(GENERATED).unwrap();
        assert!(out.contains("pub mod greeter_ulo"), "{out}");
        for (marker, arg, install) in [
            ("Greet", "super::GreetRequest", "message"),
            ("GreetMany", "super::GreetRequest", "message"),
            (
                "GreetAll",
                "tonic::Streaming<super::GreetRequest>",
                "stream",
            ),
            (
                "Converse",
                "tonic::Streaming<super::GreetRequest>",
                "stream",
            ),
        ] {
            assert!(
                out.contains(&format!("pub struct {marker};")),
                "{marker}: {out}"
            );
            assert!(
                out.contains(&format!("impl ::ulo_grpc::MethodShape for {marker}")),
                "{marker}: {out}"
            );
            assert!(
                out.contains(&format!("type Arg = {arg};")),
                "{marker}: {out}"
            );
            assert!(out.contains(&format!("::ulo_grpc::shape::{install}(request, ctx)")));
        }
        assert!(
            !out.contains("greeter_client"),
            "the client module declares nothing"
        );
    }

    #[test]
    fn a_second_run_replaces_its_own_section() {
        let once = with_companion(GENERATED).unwrap();
        let twice = with_companion(&once).unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.matches("pub mod greeter_ulo").count(), 1);
    }

    #[test]
    fn a_file_without_a_service_is_refused_rather_than_rewritten() {
        assert!(matches!(
            with_companion("pub struct Only {}"),
            Err(Error::NoService)
        ));
    }

    /// The same table tonic-build tests its own function against.
    #[test]
    fn the_module_name_is_what_tonic_would_write() {
        for (input, expected) in [
            ("Service", "service"),
            ("ThatHasALongName", "that_has_a_long_name"),
            ("greeter", "greeter"),
            ("ABCServiceX", "a_b_c_service_x"),
        ] {
            assert_eq!(to_snake(input), expected);
        }
    }

    #[test]
    fn a_method_names_its_marker_in_upper_camel() {
        assert_eq!(to_upper_camel("greet"), "Greet");
        assert_eq!(to_upper_camel("greet_all"), "GreetAll");
        assert_eq!(to_upper_camel("r#type"), "Type");
        assert_eq!(to_upper_camel("get__id"), "GetId");
    }
}
