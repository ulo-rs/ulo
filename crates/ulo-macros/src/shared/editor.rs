//! The hidden import through which an editor resolves a marker attribute a macro removes.
//!
//! `#[routes]` reads and removes `#[guards]`, `#[interceptors]`, `#[error_handlers]` and
//! `#[meta]`, and `#[injectable]` reads and removes `#[construct]` and a field's
//! `#[injectable(default)]`, so rustc never resolves those paths; a field could not carry an
//! attribute macro anyway. An editor finds what a written token means by following it into the
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
//! one `use` per marker name, listing every written occurrence's last path segment, the token as
//! written, so hover and go-to-definition on each one answer for the macro of that name. An
//! underscore import binds no name, so two occurrences, or a user's own item, module or attribute
//! named `guards`, never clash with it. The crate is named `::ulo`, as every other path the macros
//! write names it, so the import resolves wherever the rest of the expansion does, a renamed
//! dependency or a re-exporting crate brought in as `extern crate .. as ulo` included.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::shared::ulo;

/// The hidden `const` for the markers `written`, each the last path segment of one removed
/// attribute; empty when there are none.
pub(crate) fn marker_imports(written: &[Ident]) -> TokenStream {
    if written.is_empty() {
        return TokenStream::new();
    }
    imports(&ulo(), written)
}

fn imports(root: &TokenStream, written: &[Ident]) -> TokenStream {
    let mut names: Vec<(String, Vec<&Ident>)> = Vec::new();
    for ident in written {
        let name = ident.to_string();
        match names.iter_mut().find(|(seen, _)| *seen == name) {
            Some((_, occurrences)) => occurrences.push(ident),
            None => names.push((name, vec![ident])),
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
    quote!(const _: () = { #(#uses)* };)
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

    fn idents(text: &str) -> Vec<Ident> {
        let ts: TokenStream = text.parse().unwrap();
        ts.into_iter().filter_map(|tt| if let TokenTree::Ident(i) = tt { Some(i) } else { None }).collect()
    }

    fn uses(ts: &TokenStream) -> usize {
        ts.to_string().matches("use ").count()
    }

    #[test]
    fn one_use_per_name_carries_every_written_token() {
        let written = idents("guards meta\nguards construct");
        let out = imports(&quote!(::ulo), &written);
        assert_eq!(uses(&out), 3, "{out}");
        assert_eq!(occurrences(&out, "guards"), [(1, 0), (2, 0)], "each written token, at its own position");
        let text = out.to_string();
        assert!(text.contains("use :: ulo :: { guards as _ , guards as _ }"), "{text}");
        assert!(text.contains("use :: ulo :: meta as _"), "{text}");
        assert!(text.contains("use :: ulo :: construct as _"), "{text}");
    }

    /// The hidden `const` a macro wrote after its item, as text.
    fn hidden(expansion: &TokenStream) -> String {
        let text = expansion.to_string();
        text.find("const _ : () = { # [allow (unused_imports)]").map(|at| text[at..].to_owned()).unwrap_or_default()
    }

    #[test]
    fn routes_imports_every_marker_it_removes() {
        let item = quote! {
            #[guards(A)]
            #[ulo::meta(1)]
            impl X {
                #[guards(B)]
                #[http::get("/a")]
                #[cfg_attr(feature = "x", interceptors(C))]
                fn a(&self) {}
                #[foo::guards(D)]
                #[http::get("/b")]
                #[error_handlers(E)]
                #[meta(2)]
                fn b(&self) {}
            }
        };
        let out = hidden(&crate::routes::expand(TokenStream::new(), item).unwrap());
        assert_eq!(out.matches("use ").count(), 4, "{out}");
        assert!(out.contains("use :: ulo :: { guards as _ , guards as _ , guards as _ }"), "{out}");
        assert!(out.contains("use :: ulo :: { meta as _ , meta as _ }"), "{out}");
        assert!(out.contains("use :: ulo :: interceptors as _"), "{out}");
        assert!(out.contains("use :: ulo :: error_handlers as _"), "{out}");
    }

    #[test]
    fn injectable_imports_its_construct_and_field_markers() {
        let impl_form = quote!(impl X { #[construct] fn build() -> Self { X } });
        let out = hidden(&crate::injectable::expand(TokenStream::new(), impl_form).unwrap());
        assert!(out.contains("use :: ulo :: construct as _"), "{out}");
        let struct_form = quote!(struct X { #[injectable(default)] a: u8, #[ulo::injectable(default)] b: u8, c: Dep });
        let out = hidden(&crate::injectable::expand(TokenStream::new(), struct_form).unwrap());
        assert!(out.contains("use :: ulo :: { injectable as _ , injectable as _ }"), "{out}");
    }

    #[test]
    fn an_item_without_markers_gets_no_import() {
        let out = crate::routes::expand(TokenStream::new(), quote!(impl X { #[http::get("/a")] fn a(&self) {} })).unwrap();
        assert_eq!(hidden(&out), "");
        let out = crate::injectable::expand(TokenStream::new(), quote!(struct X { a: Dep })).unwrap();
        assert_eq!(hidden(&out), "");
    }

    #[test]
    fn nothing_is_written_for_no_marker() {
        assert!(marker_imports(&[]).is_empty());
    }

    #[test]
    fn the_import_names_the_crate_as_the_rest_of_the_expansion_does() {
        assert!(marker_imports(&idents("guards")).to_string().contains(":: ulo :: guards as _"));
    }
}
