//! The struct form: every field is an injection point, read in declaration order, except a field
//! marked `#[injectable(default)]`, set from `Default::default()`; on a `#[controller]` struct the
//! marker is `#[controller(default)]`, and the other attribute's marker is refused, pointing at
//! it. Tuple and unit structs are accepted; a tuple field is labelled by its index. A field behind
//! a `#[cfg]` that is off is neither declared, read nor set, whichever its role.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Field, Fields, Index, ItemStruct, Member, Type};

use ulo_handler_codegen::cfg::presence_gates;
use ulo_handler_codegen::restore::restore_item;

use crate::construct::{ConstructArgs, Kind};
use crate::shared::attrs;
use crate::shared::dependencies::{self, DependencyLabel, DependencySpec};
use crate::shared::{ConstructImpl, combine, editor, ulo};

/// The struct as written, minus its `#[injectable(default)]` or `#[controller(default)]` field
/// markers, followed by its `Construct` impl, the controller mark for `#[controller]`, and the
/// hidden import of each marker removed (`crate::shared::editor`).
pub(crate) fn expand(kind: Kind, args: ConstructArgs, mut item: ItemStruct, original: &TokenStream) -> syn::Result<TokenStream> {
    let ulo = ulo();
    let markers = item.fields.iter().flat_map(|field| &field.attrs).filter(|attr| {
        attrs::is(attr, Kind::Injectable.name()) || attrs::is(attr, Kind::Controller.name())
    });
    if let Some(e) = combine(markers.filter_map(|attr| attrs::check_marker(attr.path()).err()).collect()) {
        return Err(e);
    }
    let written: Vec<syn::Path> = item
        .fields
        .iter()
        .flat_map(|field| &field.attrs)
        .filter(|attr| attrs::is(attr, kind.name()))
        .map(|attr| attr.path().clone())
        .collect();
    let imports = editor::marker_imports(&written);
    let mut dependency_specs = Vec::new();
    let mut dependency_members = Vec::new();
    let mut default_members = Vec::new();
    let mut positional = Vec::new();
    let mut gated = false;
    let mut errors = Vec::new();

    for (index, field) in item.fields.iter_mut().enumerate() {
        let member = match &field.ident {
            Some(ident) => Member::Named(ident.clone()),
            None => Member::Unnamed(Index { index: index as u32, span: field.ty.span() }),
        };
        match classify(kind, field, index) {
            Ok(FieldRole::Dependency(spec)) => {
                gated |= !spec.gates.is_empty();
                positional.push(Positional::Dependency(dependency_specs.len()));
                dependency_specs.push(*spec);
                dependency_members.push(member);
            }
            Ok(FieldRole::Default(gates)) => {
                gated |= !gates.is_empty();
                default_members.push(quote!(#(#gates)* #member));
                positional.push(Positional::Default(gates));
            }
            Err(e) => errors.push(e),
        }
    }
    if let Some(e) = combine(errors) {
        return Err(e);
    }

    let scope = args.scope.as_ref().map_or_else(|| quote!(#ulo::scope::Auto), |s| s.path());
    let bindings = dependencies::bindings(dependency_specs.len());
    let reads = dependencies::read(&dependency_specs, &bindings);
    let dependency_members = dependency_specs.iter().zip(&dependency_members).map(|(spec, member)| {
        let gates = &spec.gates;
        quote!(#(#gates)* #member)
    });
    // A tuple field compiled out renumbers the fields after it, so a gated tuple struct is built
    // by position, where each value's `#[cfg]` drops it along with its field.
    let value = if gated && matches!(item.fields, Fields::Unnamed(_)) {
        let values = positional.iter().map(|field| match field {
            Positional::Dependency(i) => {
                let gates = &dependency_specs[*i].gates;
                let binding = &bindings[*i];
                quote!(#(#gates)* #binding)
            }
            Positional::Default(gates) => quote!(#(#gates)* ::core::default::Default::default()),
        });
        quote!(Self(#(#values),*))
    } else {
        quote! {
            Self {
                #(#dependency_members: #bindings,)*
                #(#default_members: ::core::default::Default::default(),)*
            }
        }
    };
    let construct = quote! {
        #reads
        ::core::result::Result::Ok(#value)
    };

    let name = &item.ident;
    let (_, ty_generics, _) = item.generics.split_for_impl();
    let self_ty: Type = syn::parse_quote!(#name #ty_generics);
    let construct_impl = ConstructImpl {
        self_ty: &self_ty,
        generics: &item.generics,
        dependencies: dependencies::declare(&dependency_specs, &scope),
        scope,
        timeout: args.timeout.as_ref(),
        construct,
        construct_span: name.span(),
    }
    .emit();
    let mark = kind.mark(&self_ty, &item.generics);

    let item = restore_item(original, &item);
    Ok(quote! {
        #item
        #construct_impl
        #mark
        #imports
    })
}

/// A field's place in a tuple struct built by position: the index of its dependency, or a default
/// under the field's `#[cfg]`s.
enum Positional {
    Dependency(usize),
    Default(Vec<syn::Attribute>),
}

/// What one field contributes: a dependency, or a default under the field's `#[cfg]`s.
pub(crate) enum FieldRole {
    Dependency(Box<DependencySpec>),
    Default(Vec<syn::Attribute>),
}

/// Reads and strips the field's `#[injectable(default)]`, or `#[controller(default)]` under
/// `#[controller]`; any other argument there, or the other attribute's marker, is a span error on
/// it.
pub(crate) fn classify(kind: Kind, field: &mut Field, index: usize) -> syn::Result<FieldRole> {
    let other = match kind {
        Kind::Injectable => Kind::Controller,
        Kind::Controller => Kind::Injectable,
    };
    if let Some(foreign) = field.attrs.iter().find(|attr| attrs::is(attr, other.name())) {
        return Err(syn::Error::new_spanned(
            foreign,
            format!(
                "this struct is #[{kind}]: a field set from `Default` is marked `#[{kind}(default)]`",
                kind = kind.name()
            ),
        ));
    }
    let markers = attrs::take_all(&mut field.attrs, kind.name());
    let mut default = false;
    for marker in &markers {
        let arg: syn::Ident = marker.parse_args().map_err(|_| marker_error(kind, marker))?;
        if arg != "default" {
            return Err(marker_error(kind, marker));
        }
        if default {
            return Err(syn::Error::new_spanned(marker, format!("`#[{}(default)]` is written once", kind.name())));
        }
        default = true;
    }
    let gates = presence_gates(&field.attrs);
    if default {
        return Ok(FieldRole::Default(gates));
    }
    let label = match &field.ident {
        Some(ident) => DependencyLabel::Field(ident.clone()),
        None => DependencyLabel::Index(index),
    };
    Ok(FieldRole::Dependency(Box::new(DependencySpec { label, ty: field.ty.clone(), span: field.ty.span(), gates })))
}

fn marker_error(kind: Kind, marker: &syn::Attribute) -> syn::Error {
    syn::Error::new_spanned(
        marker,
        format!(
            "a field takes `#[{}(default)]` alone, which sets it from `Default`; every other field is injected",
            kind.name()
        ),
    )
}
