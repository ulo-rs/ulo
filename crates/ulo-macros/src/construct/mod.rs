//! `#[injectable]` and `#[controller]`: the struct form, whose fields are injection points, and
//! the impl form, whose constructor's parameters are injection points. Both write a `Construct`
//! impl (§4, §5); `#[controller]` also writes `impl Controller`, the mark of a controller, which
//! holds where the type's `Handlers` impl does.

mod args;
mod impl_form;
mod struct_form;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Item, LitStr};

pub(crate) use args::ConstructArgs;

use crate::shared::ulo;

/// Which attribute is expanding. The two write the same `Construct` impl and differ in the mark,
/// in the name of the field marker their struct form reads, and in their wording.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Injectable,
    Controller,
}

impl Kind {
    /// The attribute's name, which is also the name of the struct form's field marker,
    /// `#[injectable(default)]` or `#[controller(default)]`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Injectable => "injectable",
            Kind::Controller => "controller",
        }
    }

    /// On a controller, the mark `impl Controller for Self`, which has no methods: a module
    /// mounts the `Handlers` impl `#[handlers]` writes. Nothing on an injectable.
    ///
    /// The impl holds where `Self: Handlers` holds. For a concrete type with no `Handlers` impl
    /// that bound is false where it is written, so the type fails to compile at `#[controller]`,
    /// naming `#[handlers]`, whether or not a module registers it. A generic controller whose
    /// `#[handlers]` impl is bounded more narrowly than the struct is a controller for each type
    /// that impl covers, and registers as one for those types.
    pub(crate) fn mark(self, self_ty: &syn::Type, generics: &syn::Generics) -> TokenStream {
        if self == Kind::Injectable {
            return TokenStream::new();
        }
        let ulo = ulo();
        let (impl_generics, _, _) = generics.split_for_impl();
        let mut where_clause = generics.where_clause.clone().unwrap_or_else(|| syn::parse_quote!(where));
        where_clause.predicates.push(syn::parse_quote!(#self_ty: #ulo::Handlers));
        quote! {
            impl #impl_generics #ulo::Controller for #self_ty #where_clause {}
        }
    }
}

pub(crate) fn expand(kind: Kind, attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if kind == Kind::Controller
        && let Some(prefix) = route_prefix(&attr)
    {
        return Err(syn::Error::new(
            prefix.span(),
            "#[controller] takes no route prefix: the module mounts the controller under one, \
             `m.controller::<T>().at(\"/prefix\")`",
        ));
    }
    let args: ConstructArgs = syn::parse2(attr)?;
    let original = item.clone();
    match syn::parse2::<Item>(item)? {
        Item::Struct(item) => struct_form::expand(kind, args, item, &original),
        Item::Impl(item) => impl_form::expand(kind, args, item, &original),
        other => Err(syn::Error::new_spanned(
            other,
            format!(
                "#[{}] goes on a struct, whose fields are injected, or on an impl block holding its constructor",
                kind.name()
            ),
        )),
    }
}

/// A string literal among `attr`'s arguments, `#[controller("/users")]` or
/// `#[controller(singleton, "/users")]`.
fn route_prefix(attr: &TokenStream) -> Option<LitStr> {
    attr.clone().into_iter().find_map(|token| syn::parse2::<LitStr>(token.into()).ok())
}
