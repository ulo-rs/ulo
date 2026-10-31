//! One HTTP handler attribute over `ulo-handler-codegen`.
//!
//! The expansion is the method, its `+ use<>` rewrite applied, and the three items the
//! `__handler` protocol owes (`ulo_handler_codegen::emit::MountFn`), the handler value being the
//! public `ulo_http::HttpHandler` a hand-written controller builds:
//!
//! ```text
//! let __ulo_h = ::ulo_http::HttpHandler::new(::ulo_http::Method::GET, __ulo_call);
//! let __ulo_h = (&&::ulo_http::__private::PathProbe::<P0>::new()).__ulo_path(__ulo_h);
//! let __ulo_h = (&&::ulo_http::__private::HostProbe::<P0>::new()).__ulo_host(__ulo_h);
//! /* one pair per parameter */
//! ```
//!
//! with `ViaPath`, `NotPath`, `ViaHost` and `NotHost` imported anonymously, each probe calling
//! `reads_path::<T>()` or `reads_host::<T>()` for the parameter types that read one, and the route
//! `"/users/{id}"` as `HandlerSpec::route`, which the router reads.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{Ident, ImplItemFn, LitStr};
use ulo_handler_codegen::emit::{self, MountFn};
use ulo_handler_codegen::params::{self, HandlerSig};
use ulo_handler_codegen::{Paths, protocol, reply};

/// The transport's key, which equals `<ulo_http::Http as ulo::Transport>::KEY`.
pub(crate) const KEY: &str = "http";

/// `#[<method>("<pattern>")]` on `item`. `method` is the HTTP method's name as `http::Method`'s
/// associated constant spells it.
pub(crate) fn expand(method: &'static str, attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attr_name = method.to_ascii_lowercase();
    if attr.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            format!("#[{attr_name}] takes the route pattern, as in #[{attr_name}(\"/users/{{id}}\")]"),
        ));
    }
    let pattern: LitStr = syn::parse2(attr)?;
    if let Err(reason) = crate::pattern::check(&pattern.value()) {
        return Err(syn::Error::new(pattern.span(), format!("invalid route pattern: {reason}")));
    }
    let mut item: ImplItemFn = syn::parse2(item)?;
    let Some(tokens) = protocol::take_handler_attr(&mut item.attrs)? else {
        return Err(protocol::outside_routes(&attr_name));
    };
    let sig = params::analyze(&item.sig)?;
    reply::rewrite_opaque_returns(&mut item.sig);

    let paths = Paths::new("ulo_http", "Http");
    let mount = MountFn {
        tokens: &tokens,
        sig: &sig,
        key: KEY,
        paths: &paths,
        handler_value: handler_value(method, &sig, &paths),
        route: Some(quote!(#pattern)),
        shape: None,
    }
    .emit();

    Ok(quote! {
        #item
        #mount
    })
}

/// The `HttpHandler` the mount function hands `HandlerSpec::new`: the method, the call closure,
/// and one `Path<T>` probe and one `Host<T>` probe per parameter, spanned at its type. A generic
/// handler never reaches this value: `MountFn::emit` refuses it.
fn handler_value(method: &str, sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let this = &paths.this;
    let method = Ident::new(method, Span::call_site());
    let call = emit::call_ident();
    let handler = Ident::new("__ulo_h", Span::mixed_site());
    let checks = sig.params.iter().map(|param| {
        let ty = &param.ty;
        quote_spanned! {param.span=>
            let #handler = (&&#this::__private::PathProbe::<#ty>::new()).__ulo_path(#handler);
            let #handler = (&&#this::__private::HostProbe::<#ty>::new()).__ulo_host(#handler);
        }
    });
    quote! {
        {
            #[allow(unused_imports)]
            use #this::__private::{NotHost as _, NotPath as _, ViaHost as _, ViaPath as _};
            let #handler = #this::HttpHandler::new(#this::Method::#method, #call);
            #(#checks)*
            #handler
        }
    }
}
