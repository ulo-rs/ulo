use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Result, Token,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Parse provider_alias! macro input
/// Syntax: provider_alias!("ALIAS_TOKEN", "EXISTING_TOKEN")
/// or provider_alias!(AliasType, ExistingType)
///
/// This creates an alias that points to an existing provider,
/// similar to NestJS's useExisting pattern.
/// Note: Scope cannot be overridden on aliases.
/// The alias inherits the scope from the target provider.
pub struct ProviderAliasInput {
    pub alias_token: TokenType,
    pub existing_token: TokenType,
}

impl Parse for ProviderAliasInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let alias_token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let existing_token: TokenType = input.parse()?;

        Ok(ProviderAliasInput {
            alias_token,
            existing_token,
        })
    }
}

pub fn handle_provider_alias(input: TokenStream) -> Result<TokenStream> {
    let ProviderAliasInput {
        alias_token,
        existing_token,
    } = syn::parse2(input)?;

    // Generate token expressions for runtime
    let alias_token_expr = alias_token.to_token_expr();
    let existing_token_expr = existing_token.to_token_expr();

    // Generate unique struct names based on alias token
    let sanitized_name = alias_token.sanitized_ident();
    let provider_name = format_ident!("__UloAliasProvider_{}", sanitized_name);
    let factory_name = format_ident!("__UloAliasProviderFactory_{}", sanitized_name);

    // Generate the provider struct and implementation
    let expanded = quote! {
        {
            // Alias provider struct that references another provider
            #[derive(Clone)]
            struct #provider_name {
                target_provider: std::sync::Arc<Box<dyn ulo::spi::Provider>>,
            }

            struct #factory_name;

            // Implement Provider for the alias provider wrapper
            #[ulo::async_trait]
            impl ulo::spi::Provider for #provider_name {
                fn token(&self) -> String {
                    #alias_token_expr
                }


                fn scope(&self) -> ulo::di::ProviderScope {
                    // Inherit scope from target provider
                    self.target_provider.scope()
                }

                async fn resolve(
                    &self,
                    ctx: ulo::di::Execution,
                ) -> Box<dyn std::any::Any + Send> {
                    self.target_provider.resolve(ctx).await
                }
            }

            #[ulo::async_trait]
            impl ulo::spi::ProviderFactory for #factory_name {
                fn token(&self) -> String {
                    #alias_token_expr
                }

                fn dependency_tokens(&self) -> Vec<String> {
                    vec![#existing_token_expr]
                }

                async fn build(
                    &self,
                    deps: ulo::FxHashMap<String, ulo::spi::Injectable>,
                ) -> ulo::spi::Injectable {
                    let existing_token = #existing_token_expr;
                    let ulo::spi::Injectable { instance: target_provider, roles } = deps
                        .get(&existing_token)
                        .cloned()
                        .unwrap_or_else(|| panic!(
                            "Provider alias target not found: {}. Make sure the target provider is registered before the alias.",
                            existing_token
                        ));

                    ulo::spi::Injectable::new(
                        std::sync::Arc::new(
                            Box::new(#provider_name { target_provider }) as Box<dyn ulo::spi::Provider>
                        ),
                        roles,
                    )
                }
            }

            #factory_name
        }
    };

    Ok(expanded)
}
