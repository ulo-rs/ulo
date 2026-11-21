//! `#[module(..)]`: a `Module` impl whose `register` calls the value API (§4, §8.1).
//!
//! A unit struct is identified by type (`ModuleIdentity::of_type::<Self>()`); a struct with
//! fields is a configured module, identified by value (`ModuleIdentity::of_value(self)`, which
//! needs `Eq + Hash + Clone`). Every field's secrets are registered with `m.secret(..)`, decided
//! by the field's type through the probe in `ulo::__private::secret_field`: a `Secret<_>` field
//! however its type is spelled, and an `Option`, `Vec` or map over one. A secret inside another
//! type the field holds is not reached. A field behind a `#[cfg]` is probed under that `cfg`; on a
//! tuple struct only the last field may carry one, since a field compiled out renumbers the fields
//! after it.
//! `register` never returns early: `expr?` lowers to `m.try_value(expr)`.
//!
//! `register` writes, in order: `global`, the secrets, the imports, the providers, the
//! controllers, the exports. Imports and providers keep the order written, which is part of
//! collection order.
//!
//! A `controllers` entry must implement `ulo::Handlers`, and through it `ulo::Controller`, the
//! mark of a controller, which the entry's `m.controller::<T>()` call, spanned at the entry,
//! requires. An entry under `providers` must not implement `Controller`, which the probe in
//! `ulo::__private::controller_entry` checks at the entry. A controller may be exported for another controller to read, so `exports` is not
//! checked.

mod args;
mod providers;

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Ident, Index, ItemStruct, Member};

use ulo_handler_codegen::cfg::presence_gates;
use ulo_handler_codegen::restore::restore_item;

use crate::shared::ulo;

pub(crate) use args::{ExportEntry, ModuleArgs};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: ModuleArgs = syn::parse2(attr)?;
    let original = item.clone();
    let item: ItemStruct = syn::parse2(item)?;
    expand_struct(args, item, &original)
}

/// The `ModuleDef` parameter of the generated `register`. Mixed-site, so no name in a providers
/// expression can shadow it.
pub(crate) fn module_def_param() -> Ident {
    Ident::new("m", Span::mixed_site())
}

/// The struct as written, followed by its `Module` impl.
pub(crate) fn expand_struct(args: ModuleArgs, item: ItemStruct, original: &TokenStream) -> syn::Result<TokenStream> {
    let ulo = ulo();
    let m = module_def_param();
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();

    let identity = if item.fields.is_empty() {
        quote!(#ulo::ModuleIdentity::of_type::<Self>())
    } else {
        let this = quote!(self);
        quote_spanned!(name.span()=> #ulo::ModuleIdentity::of_value(#this))
    };
    let global = args.global.then(|| quote!(#m.global();));
    let secrets = secret_fields(&item)?;
    let imports = args.imports.iter().map(|import| {
        quote_spanned! {import.span()=>
            #m.import(#import);
        }
    });
    let provided = providers::provided_types(&args.providers);
    let providers = args.providers.iter().map(|entry| entry.lower(&provided));
    let controllers = args.controllers.iter().map(|controller| {
        quote_spanned! {controller.span()=>
            #m.controller::<#controller>();
        }
    });
    let exports = args.exports.iter().map(|export| match export {
        ExportEntry::Export { ty, qualifier: None } => quote_spanned! {ty.span()=>
            #m.export::<#ty>();
        },
        ExportEntry::Export { ty, qualifier: Some(q) } => quote_spanned! {ty.span()=>
            #m.export_qualified::<#ty, #q>();
        },
        ExportEntry::Reexport { ty, qualifier: None } => quote_spanned! {ty.span()=>
            #m.reexport::<#ty>();
        },
        ExportEntry::Reexport { ty, qualifier: Some(q) } => quote_spanned! {ty.span()=>
            #m.reexport_qualified::<#ty, #q>();
        },
    });

    let item = restore_item(original, &item);
    Ok(quote! {
        #item

        impl #impl_generics #ulo::Module for #name #ty_generics #where_clause {
            fn identity(&self) -> #ulo::ModuleIdentity {
                #identity
            }

            #[allow(unused_variables)]
            fn register(&self, #m: &mut #ulo::ModuleDef<'_>) {
                #global
                #secrets
                #(#imports)*
                #(#providers)*
                #(#controllers)*
                #(#exports)*
            }
        }
    })
}

/// The secret probe on every field (`ulo::__private::secret_field`): a field whose type holds
/// secrets registers each with `m.secret(..)`, and any other registers nothing. Each probe carries
/// its field's `#[cfg]`s. A gated tuple field followed by another field is a span error on it.
pub(crate) fn secret_fields(item: &ItemStruct) -> syn::Result<TokenStream> {
    let ulo = ulo();
    let m = module_def_param();
    // `self` keeps the call-site span of the `&self` the generated `register` declares; the call
    // around it is spanned at the field, where a `Secret<T>` whose `T` is not `Display` fails.
    let this = quote!(self);
    let count = item.fields.len();
    let mut calls = Vec::with_capacity(count);
    for (index, field) in item.fields.iter().enumerate() {
        let gates = presence_gates(&field.attrs);
        let member = match &field.ident {
            Some(ident) => Member::Named(ident.clone()),
            None if !gates.is_empty() && index + 1 < count => {
                return Err(syn::Error::new_spanned(
                    &gates[0],
                    "a `#[cfg]` on a tuple module's field renumbers the fields after it; name the fields, or gate the last field alone",
                ));
            }
            None => Member::Unnamed(Index { index: index as u32, span: field.ty.span() }),
        };
        calls.push(quote_spanned! {field.ty.span()=>
            #(#gates)*
            (&#ulo::__private::secret_field::Probe(&#this.#member)).__ulo_register_secret(#m);
        });
    }
    if item.fields.is_empty() {
        return Ok(TokenStream::new());
    }
    Ok(quote! {
        {
            use #ulo::__private::secret_field::{PlainArm as _, SecretArm as _};
            #(#calls)*
        }
    })
}
