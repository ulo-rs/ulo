//! `#[construct]` outside an `#[injectable]` or `#[controller]` impl or struct: the attribute on
//! the impl or the struct strips the marker before the compiler sees it, so reaching this
//! expansion means it was written on an item where it means nothing. On a field of any other
//! struct rustc refuses it without expanding it, since an attribute macro cannot go on a field.

use proc_macro2::{Span, TokenStream};

pub(crate) fn expand(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        "#[construct] marks the constructor inside an #[injectable] or #[controller] impl block, and \
         #[construct(default)] a field of an #[injectable] or #[controller] struct",
    ))
}
