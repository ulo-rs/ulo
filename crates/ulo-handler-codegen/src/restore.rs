//! Puts the input's own attribute groups back into an expansion that went through `syn`.
//!
//! `syn` writes every delimiter group with one span, so an attribute that survives a parse and a
//! `quote!` reaches the compiler with its `[..]` and `(..)` delimiters collapsed, and an editor can
//! no longer map the attribute's path to the source it was written at. [`restore_attrs`] replaces
//! each surviving `#[..]` group with the group the input carried, which is the same tokens with
//! their original spans.
//!
//! A group is matched by its contents with spacing ignored and by its span, which `syn` keeps
//! from the input as the span of the whole group: an attribute the macro removed, rewrote or
//! generated has no counterpart and is left as built, even where its text equals another
//! attribute's. Among groups equal in both, which only an input that repeats one source position
//! can carry, source order decides.

use std::collections::{HashMap, VecDeque};

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;

/// `output` with every `#[..]` or `#![..]` group that also appears in `input` replaced by the
/// input's group.
pub fn restore_attrs(input: &TokenStream, output: TokenStream) -> TokenStream {
    let mut originals = Originals::default();
    originals.collect(input);
    originals.restore(output)
}

/// `item` written back as tokens, with the attribute groups `input` carried restored. `input` is
/// the tokens the macro was given and `item` what it parsed from them and may have changed; only
/// the item is restored, so an attribute the macro generates around it is not matched.
pub fn restore_item(input: &TokenStream, item: &impl ToTokens) -> TokenStream {
    restore_attrs(input, item.to_token_stream())
}

/// The attribute groups of an input, queued per [`key`] in source order.
#[derive(Default)]
struct Originals {
    queues: HashMap<(String, String), VecDeque<Group>>,
}

impl Originals {
    fn collect(&mut self, ts: &TokenStream) {
        let tts: Vec<TokenTree> = ts.clone().into_iter().collect();
        for (i, tt) in tts.iter().enumerate() {
            let TokenTree::Group(g) = tt else { continue };
            if is_attr_group(&tts, i) {
                self.queues.entry(key(g)).or_default().push_back(g.clone());
            } else {
                self.collect(&g.stream());
            }
        }
    }

    fn restore(&mut self, ts: TokenStream) -> TokenStream {
        let tts: Vec<TokenTree> = ts.into_iter().collect();
        let mut out = Vec::with_capacity(tts.len());
        for (i, tt) in tts.iter().enumerate() {
            let TokenTree::Group(g) = tt else {
                out.push(tt.clone());
                continue;
            };
            if is_attr_group(&tts, i)
                && let Some(original) = self.queues.get_mut(&key(g)).and_then(VecDeque::pop_front)
            {
                out.push(TokenTree::Group(original));
                continue;
            }
            let mut rebuilt = Group::new(g.delimiter(), self.restore(g.stream()));
            rebuilt.set_span(g.span());
            out.push(TokenTree::Group(rebuilt));
        }
        out.into_iter().collect()
    }
}

/// A group's flattened contents and its span. A span offers no comparison, so its `Debug` form
/// stands in: the compiler writes the position and the syntax context there. Outside the compiler
/// every span prints alike, unless `proc-macro2`'s `span-locations` is on, and the contents alone
/// decide.
fn key(g: &Group) -> (String, String) {
    (flatten(&g.stream()), format!("{:?}", g.span()))
}

/// Whether `tts[i]` is the `[..]` of an outer `#[..]` or an inner `#![..]` attribute.
fn is_attr_group(tts: &[TokenTree], i: usize) -> bool {
    let TokenTree::Group(g) = &tts[i] else { return false };
    if g.delimiter() != Delimiter::Bracket || i == 0 {
        return false;
    }
    let is_punct = |tt: &TokenTree, c: char| matches!(tt, TokenTree::Punct(p) if p.as_char() == c);
    is_punct(&tts[i - 1], '#') || (i >= 2 && is_punct(&tts[i - 1], '!') && is_punct(&tts[i - 2], '#'))
}

/// The tokens as one string with no spacing information, delimiters included. A `None`-delimited
/// group is transparent, since `syn` and `quote!` disagree on where those appear.
fn flatten(ts: &TokenStream) -> String {
    let mut s = String::new();
    flatten_into(ts, &mut s);
    s
}

fn flatten_into(ts: &TokenStream, s: &mut String) {
    for tt in ts.clone() {
        match tt {
            TokenTree::Group(g) => match g.delimiter() {
                Delimiter::None => flatten_into(&g.stream(), s),
                d => {
                    let (open, close) = match d {
                        Delimiter::Parenthesis => ('(', ')'),
                        Delimiter::Brace => ('{', '}'),
                        _ => ('[', ']'),
                    };
                    s.push(open);
                    flatten_into(&g.stream(), s);
                    s.push(close);
                }
            },
            other => {
                s.push_str(&other.to_string());
                s.push(' ');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proc_macro2::{Punct, Spacing};
    use quote::quote;

    use super::*;

    /// A copy of `ts` in which every comma is `Joint`: the same tokens as far as `flatten` can
    /// tell, distinguishable from the original through the punct's spacing.
    fn joint_commas(ts: TokenStream) -> TokenStream {
        ts.into_iter()
            .map(|tt| match tt {
                TokenTree::Group(g) => {
                    let mut n = Group::new(g.delimiter(), joint_commas(g.stream()));
                    n.set_span(g.span());
                    TokenTree::Group(n)
                }
                TokenTree::Punct(p) if p.as_char() == ',' => TokenTree::Punct(Punct::new(',', Spacing::Joint)),
                other => other,
            })
            .collect()
    }

    /// The comma spacings inside the attribute groups of `ts`, in order, `a` for `Alone`.
    fn attr_commas(ts: &TokenStream) -> String {
        fn walk(ts: &TokenStream, in_attr: bool, out: &mut String) {
            let tts: Vec<TokenTree> = ts.clone().into_iter().collect();
            for (i, tt) in tts.iter().enumerate() {
                match tt {
                    TokenTree::Group(g) => walk(&g.stream(), in_attr || is_attr_group(&tts, i), out),
                    TokenTree::Punct(p) if in_attr && p.as_char() == ',' => {
                        out.push(if p.spacing() == Spacing::Joint { 'j' } else { 'a' });
                    }
                    _ => {}
                }
            }
        }
        let mut out = String::new();
        walk(ts, false, &mut out);
        out
    }

    fn restored(input: TokenStream) -> TokenStream {
        restore_attrs(&input, joint_commas(input.clone()))
    }

    #[test]
    fn flatten_ignores_spacing_and_keeps_delimiters() {
        let spaced: TokenStream = "a :: b ( x , y )".parse().unwrap();
        let tight: TokenStream = "a::b(x,y)".parse().unwrap();
        assert_eq!(flatten(&spaced), flatten(&tight));
        assert_eq!(flatten(&"f(x)".parse().unwrap()), "f (x )");
        assert_ne!(flatten(&"f(x)".parse().unwrap()), flatten(&"f[x]".parse().unwrap()));
        assert_ne!(flatten(&"f(x)".parse().unwrap()), flatten(&"f x".parse().unwrap()));
    }

    #[test]
    fn flatten_sees_through_a_none_group() {
        let wrapped: TokenStream = TokenTree::Group(Group::new(Delimiter::None, quote!(a b))).into();
        assert_eq!(flatten(&wrapped), flatten(&quote!(a b)));
    }

    #[test]
    fn a_surviving_attribute_is_the_input_group() {
        let input = quote!(#[m::get("/x", y)] fn f() {});
        assert_eq!(attr_commas(&restored(input.clone())), "a");
        assert_eq!(attr_commas(&joint_commas(input)), "j");
    }

    #[test]
    fn inner_attributes_are_restored() {
        let input = quote!(mod m { #![allow(a, b)] });
        assert_eq!(attr_commas(&restored(input)), "a");
    }

    #[test]
    fn nested_groups_are_searched_on_both_sides() {
        let input = quote!(impl X { fn f() { #[allow(a, b)] let _x = 1; } #[doc = "d"] fn g() {} });
        let out = restored(input.clone());
        assert_eq!(attr_commas(&out), "a");
        assert_eq!(out.to_string(), input.to_string());
    }

    #[test]
    fn identical_attributes_match_by_order() {
        // Same contents, told apart by the comma spacing the input gives each: the first
        // occurrence is the one restored first.
        let mut input = joint_commas(quote!(#[a(x, y)] fn f() {}));
        input.extend(quote!(#[a(x, y)] fn g() {}));
        input.extend(joint_commas(quote!(#[a(x, y)] fn h() {})));
        assert_eq!(attr_commas(&input), "jaj");
        assert_eq!(attr_commas(&restore_attrs(&input, joint_commas(input.clone()))), "jaj");
    }

    /// The spans of the attribute groups of `ts`, at any depth, in order, as they print.
    fn attr_spans(ts: &TokenStream) -> Vec<String> {
        fn walk(ts: &TokenStream, out: &mut Vec<String>) {
            let tts: Vec<TokenTree> = ts.clone().into_iter().collect();
            for (i, tt) in tts.iter().enumerate() {
                if let TokenTree::Group(g) = tt {
                    if is_attr_group(&tts, i) {
                        out.push(format!("{:?}", g.span()));
                    }
                    walk(&g.stream(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(ts, &mut out);
        out
    }

    /// Parsed from text, so that each span prints its position, as a span does in the compiler
    /// (the `span-locations` feature of the dev-dependency).
    fn located(text: &str) -> (TokenStream, syn::File) {
        let input: TokenStream = text.parse().unwrap();
        let spans = attr_spans(&input);
        assert!(spans.windows(2).all(|w| w[0] != w[1]), "spans do not print their position: {spans:?}");
        let file = syn::parse2(input.clone()).unwrap();
        (input, file)
    }

    #[test]
    fn a_removed_attribute_does_not_lend_its_group_to_an_identical_one() {
        let (input, mut file) = located("#[a(x, y)] fn f() {} #[a(x, y)] fn h() {}");
        file.items.remove(0);
        let out = restore_attrs(&input, joint_commas(file.into_token_stream()));
        assert_eq!(attr_commas(&out), "a");
        assert_eq!(attr_spans(&out), attr_spans(&input)[1..]);
    }

    #[test]
    fn a_rewritten_attribute_does_not_take_the_group_of_one_it_now_equals() {
        let (input, mut file) = located("#[a(x, y, z)] fn f() {} #[a(x, y)] fn h() {}");
        let syn::Item::Fn(f) = &mut file.items[0] else { unreachable!() };
        let syn::Meta::List(list) = &mut f.attrs[0].meta else { unreachable!() };
        list.tokens = quote!(x, y);
        let out = restore_attrs(&input, joint_commas(file.into_token_stream()));
        assert_eq!(attr_commas(&out), "ja");
    }

    #[test]
    fn a_generated_attribute_does_not_take_the_group_of_one_it_equals() {
        let (input, mut file) = located("#[g] fn f() {} #[cfg(any(x, y))] fn h() {}");
        let syn::Item::Fn(f) = &mut file.items[0] else { unreachable!() };
        f.attrs.push(syn::parse_quote!(#[handler(#[cfg(any(x, y))] g)]));
        let out = restore_attrs(&input, joint_commas(file.into_token_stream()));
        assert_eq!(attr_commas(&out), "ja");
    }

    #[test]
    fn an_attribute_without_an_original_is_left_as_built() {
        let input = quote!(#[a(x, y)] fn f() {});
        let output = joint_commas(quote!(#[a(x, y)] #[generated(p, q)] fn f() {}));
        let out = restore_attrs(&input, output);
        assert_eq!(attr_commas(&out), "aj");
    }

    #[test]
    fn a_group_that_is_not_an_attribute_is_not_restored() {
        let input = quote!(const A: [u8; 2] = [1, 2]; fn f(a: u8, b: u8) {});
        assert_eq!(attr_commas(&joint_commas(input.clone())), "");
        let out = restored(input.clone());
        let commas = |ts: &TokenStream| ts.to_string().matches(", ").count();
        assert_eq!(commas(&out), commas(&joint_commas(input)));
    }

    #[test]
    fn a_cfg_attr_body_is_part_of_the_group() {
        let input = quote!(#[cfg_attr(feature = "x", m::get("/b"), guards(A, B))] fn f() {});
        assert_eq!(attr_commas(&restored(input)), "aaa");
    }

    #[test]
    fn an_attribute_with_different_contents_is_left_alone() {
        let input = quote!(#[a(x, y)] fn f() {});
        let output = joint_commas(quote!(#[a(x, z)] fn f() {}));
        assert_eq!(attr_commas(&restore_attrs(&input, output)), "j");
    }
}
