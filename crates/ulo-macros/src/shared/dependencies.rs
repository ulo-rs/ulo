//! Dependencies as the macros see them: a field or parameter with its type, emitted twice: its
//! declaration in `Construct::dependencies`, which carries the `quote_spanned!` assertion that
//! points an error at the field or parameter itself, and its read in `Construct::construct`.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{Ident, Type};

use crate::shared::{dependencies_param, resolver_param, ulo};

pub(crate) struct DependencySpec {
    pub(crate) label: DependencyLabel,
    pub(crate) ty: Type,
    /// The span of the field's or parameter's type, where its assertion's error points.
    pub(crate) span: Span,
}

pub(crate) enum DependencyLabel {
    Field(Ident),
    /// A tuple struct's field, declared under its index, so the wiring report reads
    /// ``field `2` `` even when a defaulted field sits between two injected ones.
    Index(usize),
    Param(Ident),
}

/// `(&::ulo::__private::Site::<Ty, #scope>::new()).field(d, "name");` or `.param(..)` for each
/// dependency, spanned at its type: the call declares the dependency and asserts
/// `AllowedIn<#scope>` for it. A type that is not `FromContainer` declares nothing and asserts
/// nothing, so the read in `construct` is the one place that refuses it.
pub(crate) fn declare(dependencies: &[DependencySpec], scope: &TokenStream) -> TokenStream {
    let ulo = ulo();
    let d = dependencies_param();
    let calls = dependencies.iter().map(|dependency| {
        let ty = &dependency.ty;
        let (helper, name) = match &dependency.label {
            DependencyLabel::Field(ident) => (Ident::new("field", dependency.span), label_text(ident)),
            DependencyLabel::Index(index) => (Ident::new("field", dependency.span), index.to_string()),
            DependencyLabel::Param(ident) => (Ident::new("param", dependency.span), label_text(ident)),
        };
        quote_spanned! {dependency.span=>
            {
                #[allow(unused_imports)]
                use #ulo::__private::{Declare as _, Declined as _};
                (&#ulo::__private::Site::<#ty, #scope>::new()).#helper(#d, #name);
            }
        }
    });
    quote!(#(#calls)*)
}

/// `let #binding = <#ty as ::ulo::FromContainer>::from_container(r).await?;` for each dependency, in
/// order. A failed read propagates as `ConstructError::Dependency` through `From<LookupError>`.
pub(crate) fn read(dependencies: &[DependencySpec], bindings: &[Ident]) -> TokenStream {
    let ulo = ulo();
    let r = resolver_param();
    let reads = dependencies.iter().zip(bindings).map(|(dependency, binding)| {
        let ty = &dependency.ty;
        quote_spanned! {dependency.span=>
            let #binding = <#ty as #ulo::FromContainer>::from_container(#r).await?;
        }
    });
    quote!(#(#reads)*)
}

/// One binding per dependency, mixed-site so no name the user writes can collide with it.
pub(crate) fn bindings(count: usize) -> Vec<Ident> {
    (0..count).map(|i| Ident::new(&format!("__ulo_dep_{i}"), Span::mixed_site())).collect()
}

fn label_text(ident: &Ident) -> String {
    syn::ext::IdentExt::unraw(ident).to_string()
}
