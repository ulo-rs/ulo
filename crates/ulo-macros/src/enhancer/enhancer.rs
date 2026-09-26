use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, Result, Token, punctuated::Punctuated, spanned::Spanned};

fn is_enhancer(segment: &Ident) -> bool {
    matches!(
        segment.to_string().as_str(),
        "use_guards" | "use_interceptors" | "use_error_handlers"
    )
}

/// Matches by the path's last segment so path-qualified forms
/// (`#[ulo::use_guards(…)]`) are recognized alongside the bare ones.
pub fn has_enhancer_attribute(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| is_enhancer(&segment.ident))
}

/// One argument of `#[use_guards(…)]`, `#[use_interceptors(…)]` or `#[use_error_handlers(…)]`,
/// classified by its grammar. The spelling decides the lifecycle — `ulo::enhancer::GuardDeclaration`
/// or its sibling is the type each becomes — and [`enhancer_entries`] emits it.
#[derive(Clone)]
pub enum EnhancerInfo {
    /// `MyGuard` or `"AUTH_GUARD"`: a DI token, resolved at `create`. Carries the expression that
    /// produces the token string.
    Token(TokenStream),
    /// `MyGuard {}` or `MyGuard::new(..)`: a value, built at startup and shared.
    Value(TokenStream),
    /// `|ctx| ..`: a constructor, run once per execution.
    Constructor(TokenStream),
}

/// Read every enhancer attribute into one ordered list per role.
///
/// Controller-level entries come first and method-level ones append after them, each in the order
/// written, so the list is the order written. The key is the attribute name without
/// `use_`: `guards`, `interceptors`, `error_handlers`.
pub fn create_enhancer_infos(
    controller_enhancers_attr: Vec<(&Ident, &Attribute)>,
    method_enhancers_attr: Vec<(&Ident, &Attribute)>,
) -> Result<HashMap<String, Vec<EnhancerInfo>>> {
    let mut enhancers: HashMap<String, Vec<EnhancerInfo>> = HashMap::new();

    for (ident, attr) in controller_enhancers_attr
        .into_iter()
        .chain(method_enhancers_attr)
    {
        let arg_exprs = attr
            .parse_args_with(Punctuated::<syn::Expr, Token![,]>::parse_terminated)
            .map_err(|_| Error::new(attr.span(), "Invalid attribute format"))?;

        let key = ident.to_string().replace("use_", "");

        for arg_expr in arg_exprs {
            let info = extract_enhancer_info(&arg_expr)?;
            // An error handler has no per-execution arm to land on, so the refusal is here, at
            // the argument, rather than at startup.
            if key == "error_handlers" && matches!(info, EnhancerInfo::Constructor(_)) {
                return Err(Error::new(
                    arg_expr.span(),
                    "an error handler is built once and shared, so the closure form has nothing \
                     to build per execution; write a type name or a value",
                ));
            }
            enhancers.entry(key.clone()).or_default().push(info);
        }
    }

    Ok(enhancers)
}

/// Whether any role has an entry. A generator that emits a descriptor only when something is
/// declared reads this.
pub fn declares_anything(infos: &HashMap<String, Vec<EnhancerInfo>>) -> bool {
    infos.values().any(|entries| !entries.is_empty())
}

/// The declaration type one role's entries are emitted as.
fn declaration_path(key: &str) -> TokenStream {
    match key {
        "guards" => quote! { ::ulo::enhancer::GuardDeclaration },
        "interceptors" => quote! { ::ulo::enhancer::InterceptorDeclaration },
        "error_handlers" => quote! { ::ulo::enhancer::ErrorHandlerDeclaration },
        other => panic!("no declaration type for enhancer role `{other}`"),
    }
}

/// One role's entries as the declaration values the descriptor carries, in the order written.
///
/// `transport` is the `ulo::dispatch` marker the generator serves — `::ulo::dispatch::Http` and
/// its three siblings. Every entry names it, so a value has a role trait object to coerce to and
/// a closure has a context type to infer its parameter from.
pub fn enhancer_entries(
    infos: &HashMap<String, Vec<EnhancerInfo>>,
    key: &str,
    transport: &TokenStream,
) -> Vec<TokenStream> {
    let declaration = declaration_path(key);
    infos
        .get(key)
        .map(|entries| {
            entries
                .iter()
                .map(|info| match info {
                    EnhancerInfo::Token(token) => {
                        quote! { #declaration::<#transport>::Token(#token) }
                    }
                    EnhancerInfo::Value(value) => {
                        quote! { #declaration::<#transport>::value(#value) }
                    }
                    EnhancerInfo::Constructor(build) => {
                        quote! { #declaration::<#transport>::constructor(#build) }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Classify one attribute argument by its grammar.
///
/// - `MyGuard` — a bare path: a DI token, `token_of::<MyGuard>()`
/// - `"AUTH_GUARD"` — a string literal: a DI token, the string itself
/// - `MyGuard {}`, `MyGuard { role: "admin" }` — a struct literal: a value
/// - `MyGuard::new()`, `MyGuard::new("admin")` — a call: a value
/// - `|ctx| MyGuard::for_call(ctx)` — a closure: a constructor
fn extract_enhancer_info(expr: &syn::Expr) -> Result<EnhancerInfo> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit_str),
            ..
        }) => Ok(EnhancerInfo::Token(quote! { #lit_str.to_string() })),
        syn::Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
            let type_ident = &expr_path.path.segments[0].ident;
            Ok(EnhancerInfo::Token(
                quote! { ::ulo::di::token_of::<#type_ident>() },
            ))
        }
        syn::Expr::Struct(_) => Ok(EnhancerInfo::Value(quote! { #expr })),
        syn::Expr::Call(expr_call) => {
            if let syn::Expr::Path(_) = &*expr_call.func {
                return Ok(EnhancerInfo::Value(quote! { #expr }));
            }
            Err(Error::new(
                expr.span(),
                "Expected type identifier or Type::new() expression",
            ))
        }
        syn::Expr::Closure(_) => Ok(EnhancerInfo::Constructor(quote! { #expr })),
        _ => Err(Error::new(
            expr.span(),
            "Expected a type name (MyGuard), a string token (\"AUTH_GUARD\"), a struct literal (MyGuard{}), a constructor call (MyGuard::new()) or a closure (|ctx| MyGuard::new(ctx))",
        )),
    }
}

/// Collect enhancer attributes as (name, attribute) pairs in declaration order.
///
/// The name is the path's last segment, so `#[use_guards(…)]` and `#[ulo::use_guards(…)]`
/// collect identically. Stacked attributes of the same kind each get their own pair;
/// [`create_enhancer_infos`] appends them in order.
pub fn get_enhancers_attr(attrs: &[Attribute]) -> Result<Vec<(&Ident, &Attribute)>> {
    Ok(attrs
        .iter()
        .filter_map(|attr| {
            let segment = attr.path().segments.last()?;
            is_enhancer(&segment.ident).then_some((&segment.ident, attr))
        })
        .collect())
}
