//! The hidden import through which an editor resolves a marker attribute a macro removes.
//!
//! `#[handlers]` reads and removes `#[guards]`, `#[interceptors]`, `#[error_handlers]` and
//! `#[meta]`, and `#[injectable]` and `#[controller]` read and remove `#[construct]` and a field's
//! `#[injectable(default)]` or `#[controller(default)]`, so rustc never resolves those paths; a
//! field could not carry an attribute macro anyway. An editor finds what a written token means by following it into the
//! expansion by its span, and a removed attribute's tokens are nowhere in it. Beside the item, the
//! macro therefore writes
//!
//! ```text
//! const _: () = {
//!     #[allow(unused_imports)]
//!     use ::ulo::{guards as _, guards as _};
//!     #[allow(unused_imports)]
//!     use ::ulo::meta as _;
//! };
//! ```
//!
//! one `use` per marker name, listing every bare occurrence's name, the token as written, so
//! hover and go-to-definition on each one answer for the macro of that name. An occurrence written
//! `#[ulo::guards]` gets a `use` of its own, `use ::ulo::guards as _;` built from both of its
//! written tokens, so its `ulo` segment answers for the crate as well. An underscore import binds
//! no name, so two occurrences, or a user's own item, module or attribute named `guards`, never
//! clash with it. The crate is named `::ulo`, as every other path the macros write names it, and
//! `ulo` is the one qualifier a marker accepts (`crate::shared::attrs`), so the import resolves
//! wherever the rest of the expansion does, a renamed dependency or a re-exporting crate brought
//! in as `extern crate .. as ulo` included.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Path};

use crate::shared::{attrs, ulo};

/// The hidden `const` for the markers `written`, each the path of one removed attribute, bare or
/// under `ulo`; empty when there are none.
pub(crate) fn marker_imports(written: &[Path]) -> TokenStream {
    if written.is_empty() {
        return TokenStream::new();
    }
    imports(&ulo(), written)
}

fn imports(root: &TokenStream, written: &[Path]) -> TokenStream {
    let mut names: Vec<(String, Vec<&Ident>)> = Vec::new();
    let mut qualified = Vec::new();
    for path in written {
        let Some(name) = path.segments.last().map(|segment| &segment.ident) else { continue };
        if let Some(ulo) = attrs::qualifier(path) {
            qualified.push(quote! {
                #[allow(unused_imports)]
                use ::#ulo::#name as _;
            });
            continue;
        }
        match names.iter_mut().find(|(seen, _)| *name == *seen) {
            Some((_, occurrences)) => occurrences.push(name),
            None => names.push((name.to_string(), vec![name])),
        }
    }
    let uses = names.iter().map(|(_, occurrences)| {
        // Braces only around two or more, so a crate warning on `unused_import_braces` sees none.
        let tree = match occurrences.as_slice() {
            [one] => quote!(#one as _),
            many => quote!({ #(#many as _),* }),
        };
        quote! {
            #[allow(unused_imports)]
            use #root::#tree;
        }
    });
    quote!(const _: () = { #(#uses)* #(#qualified)* };)
}

#[cfg(test)]
mod tests {
    use proc_macro2::TokenTree;

    use super::*;

    /// The idents in `ts` named `name`, at any depth, with the line and column each starts at.
    fn occurrences(ts: &TokenStream, name: &str) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for tt in ts.clone() {
            match tt {
                TokenTree::Ident(ident) if ident == name => {
                    let start = ident.span().start();
                    found.push((start.line, start.column));
                }
                TokenTree::Group(group) => found.extend(occurrences(&group.stream(), name)),
                _ => {}
            }
        }
        found
    }

    /// The comma-separated paths in `text`, each keeping the line and column of its tokens.
    fn paths(text: &str) -> Vec<Path> {
        use syn::parse::Parser;
        let parse = syn::punctuated::Punctuated::<Path, syn::Token![,]>::parse_terminated;
        parse.parse_str(text).unwrap().into_iter().collect()
    }

    fn uses(ts: &TokenStream) -> usize {
        ts.to_string().matches("use ").count()
    }

    #[test]
    fn one_use_per_name_carries_every_written_token() {
        let written = paths("guards, meta,\nguards, construct");
        let out = imports(&quote!(::ulo), &written);
        assert_eq!(uses(&out), 3, "{out}");
        assert_eq!(occurrences(&out, "guards"), [(1, 0), (2, 0)], "each written token, at its own position");
        let text = out.to_string();
        assert!(text.contains("use :: ulo :: { guards as _ , guards as _ }"), "{text}");
        assert!(text.contains("use :: ulo :: meta as _"), "{text}");
        assert!(text.contains("use :: ulo :: construct as _"), "{text}");
    }

    #[test]
    fn a_marker_under_ulo_imports_through_both_written_tokens() {
        let written = paths("guards,\n  ulo::guards");
        let out = imports(&quote!(::ulo), &written);
        assert_eq!(uses(&out), 2, "{out}");
        assert_eq!(occurrences(&out, "ulo"), [(1, 0), (2, 2)], "the root for the bare one, the written `ulo` for the other");
        assert_eq!(occurrences(&out, "guards"), [(1, 0), (2, 7)]);
        assert!(out.to_string().contains("use :: ulo :: guards as _"), "{out}");
    }

    /// The hidden `const` a macro wrote after its item, as text.
    fn hidden(expansion: &TokenStream) -> String {
        let text = expansion.to_string();
        text.find("const _ : () = { # [allow (unused_imports)]").map(|at| text[at..].to_owned()).unwrap_or_default()
    }

    #[test]
    fn handlers_imports_every_marker_it_removes() {
        let item = quote! {
            #[guards(A)]
            #[ulo::meta(1)]
            impl X {
                #[guards(B)]
                #[http::get("/a")]
                #[cfg_attr(feature = "x", interceptors(C))]
                fn a(&self) {}
                #[ulo::guards(D)]
                #[http::get("/b")]
                #[error_handlers(E)]
                #[meta(2)]
                fn b(&self) {}
            }
        };
        let out = hidden(&crate::handlers::expand(TokenStream::new(), item).unwrap());
        assert_eq!(out.matches("use ").count(), 6, "{out}");
        assert!(out.contains("use :: ulo :: { guards as _ , guards as _ }"), "{out}");
        assert_eq!(out.matches("use :: ulo :: guards as _").count(), 1, "{out}");
        assert!(out.contains("use :: ulo :: meta as _"), "{out}");
        assert_eq!(out.matches("use :: ulo :: meta as _").count(), 2, "one bare, one under `ulo`: {out}");
        assert!(out.contains("use :: ulo :: interceptors as _"), "{out}");
        assert!(out.contains("use :: ulo :: error_handlers as _"), "{out}");
    }

    #[test]
    fn a_marker_under_another_qualifier_is_refused_at_its_path() {
        let refused = |item| crate::handlers::expand(TokenStream::new(), item).map(|_| ()).unwrap_err().to_string();
        let on_impl = refused(quote!(#[wrong::guards(A)] impl X { #[http::get("/a")] fn a(&self) {} }));
        assert!(on_impl.starts_with("write the marker bare, or as `ulo::guards`"), "{on_impl}");
        let on_method = refused(quote!(impl X { #[http::get("/a")] #[cfg_attr(all(), a::b::meta(1))] fn a(&self) {} }));
        assert!(on_method.starts_with("write the marker bare, or as `ulo::meta`"), "{on_method}");
        let construct = |kind, item| crate::construct::expand(kind, TokenStream::new(), item).map(|_| ()).unwrap_err().to_string();
        let ctor = construct(crate::construct::Kind::Injectable, quote!(impl X { #[framework::construct] fn build() -> Self { X } }));
        assert!(ctor.starts_with("write the marker bare, or as `ulo::construct`"), "{ctor}");
        let field = construct(crate::construct::Kind::Controller, quote!(struct X { #[wrong::controller(default)] a: u8 }));
        assert!(field.starts_with("write the marker bare, or as `ulo::controller`"), "{field}");
    }

    #[test]
    fn injectable_imports_its_construct_and_field_markers() {
        let impl_form = quote!(impl X { #[construct] fn build() -> Self { X } });
        let out = hidden(&crate::construct::expand(crate::construct::Kind::Injectable, TokenStream::new(), impl_form).unwrap());
        assert!(out.contains("use :: ulo :: construct as _"), "{out}");
        let struct_form = quote!(struct X { #[injectable(default)] a: u8, #[ulo::injectable(default)] b: u8, c: Dep });
        let out = hidden(&crate::construct::expand(crate::construct::Kind::Injectable, TokenStream::new(), struct_form).unwrap());
        assert_eq!(out.matches("use :: ulo :: injectable as _").count(), 2, "{out}");
    }

    #[test]
    fn controller_imports_its_construct_and_field_markers() {
        let controller = |item| crate::construct::expand(crate::construct::Kind::Controller, TokenStream::new(), item);
        let out = hidden(&controller(quote!(impl X { #[construct] fn build() -> Self { X } })).unwrap());
        assert!(out.contains("use :: ulo :: construct as _"), "{out}");
        let out = hidden(&controller(quote!(struct X { #[controller(default)] a: u8, #[ulo::controller(default)] b: u8, c: Dep })).unwrap());
        assert_eq!(out.matches("use :: ulo :: controller as _").count(), 2, "{out}");
    }

    #[test]
    fn an_item_without_markers_gets_no_import() {
        let out = crate::handlers::expand(TokenStream::new(), quote!(impl X { #[http::get("/a")] fn a(&self) {} })).unwrap();
        assert_eq!(hidden(&out), "");
        let out = crate::construct::expand(crate::construct::Kind::Injectable, TokenStream::new(), quote!(struct X { a: Dep })).unwrap();
        assert_eq!(hidden(&out), "");
    }

    #[test]
    fn nothing_is_written_for_no_marker() {
        assert!(marker_imports(&[]).is_empty());
    }

    #[test]
    fn the_import_names_the_crate_as_the_rest_of_the_expansion_does() {
        assert!(marker_imports(&paths("guards")).to_string().contains(":: ulo :: guards as _"));
    }
}
