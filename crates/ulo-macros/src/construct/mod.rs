//! `#[injectable]` and `#[controller]`: the struct form, whose fields are injection points, and
//! the impl form, whose constructor's parameters are injection points. Both write a `Construct`
//! impl (§4, §5); `#[controller]` also writes `ulo::__private::controller_entry::IsController`,
//! the mark `#[module]` reads to accept a type under `controllers` and to refuse it under
//! `providers`.

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

    /// `impl IsController for Self` on a controller; nothing on an injectable.
    pub(crate) fn mark(self, self_ty: &syn::Type, generics: &syn::Generics) -> TokenStream {
        if self == Kind::Injectable {
            return TokenStream::new();
        }
        let ulo = ulo();
        let (impl_generics, _, where_clause) = generics.split_for_impl();
        quote! {
            impl #impl_generics #ulo::__private::controller_entry::IsController for #self_ty #where_clause {}
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
