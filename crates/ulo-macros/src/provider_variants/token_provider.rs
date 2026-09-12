use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Result, Token, Type, TypePath,
    parse::{Parse, ParseStream},
};

use crate::shared::TokenType;

/// Parse provider_token! macro input
/// Syntax: provider_token!("TOKEN", Type)
/// or provider_token!(TOKEN_CONST, Type)
///
/// This creates a provider with a custom token, similar to NestJS's useClass pattern:
/// ```typescript
/// {
///   provide: 'CUSTOM_TOKEN',
///   useClass: SomeClass
/// }
/// ```
/// The type is registered ONLY under the custom token, NOT under its type name.
/// This is different from provider_alias! which requires the type to be pre-registered.
/// Note: Scope is inherited from the type's #[injectable(scope = "...")] attribute.
/// Scope override is NOT supported.
pub struct ProviderTokenInput {
    pub token: TokenType,
    pub provider_type: Type,
}

impl Parse for ProviderTokenInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let token: TokenType = input.parse()?;
        let _: Token![,] = input.parse()?;
        let provider_type: Type = input.parse()?;

        Ok(ProviderTokenInput {
            token,
            provider_type,
        })
    }
}

pub fn handle_provider_token(input: TokenStream) -> Result<TokenStream> {
    let ProviderTokenInput {
        token,
        provider_type,
    } = syn::parse2(input)?;

    // Generate token expression for runtime
    let token_expr = token.to_token_expr();

    let type_path = match &provider_type {
        Type::Path(TypePath { path, .. }) => path.clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                provider_type,
                "provider_token! only supports simple type paths (e.g., DatabaseService or mymodule::DatabaseService)",
            ));
        }
    };

    // Generate unique struct names based on token
    let sanitized_name = token.sanitized_ident();
    let wrapper_factory_name = format_ident!("__UloTokenProviderFactory_{}", sanitized_name);

    let expanded = quote! {
        {
            struct #wrapper_factory_name;

            #[ulo::async_trait]
            impl ulo::spi::ProviderFactory for #wrapper_factory_name {
                fn token(&self) -> String {
                    #token_expr
                }

                fn dependency_tokens(&self) -> Vec<String> {
                    #type_path::__ulo_provider_factory().dependency_tokens()
                }

                async fn build(
                    &self,
                    deps: ulo::FxHashMap<String, ulo::spi::Injectable>,
                ) -> ulo::spi::Injectable {
                    // Build inner and receive its roles — no downcast needed.
                    let ulo::spi::Injectable { instance: inner_provider, roles } = #type_path::__ulo_provider_factory().build(deps).await;

                    // Wrap under the custom token; forward roles unchanged.
                    #[derive(Clone)]
                    struct CustomTokenProvider {
                        custom_token: String,
                        inner_provider: std::sync::Arc<Box<dyn ulo::spi::Provider>>,
                    }

                    #[ulo::async_trait]
                    impl ulo::spi::Provider for CustomTokenProvider {
                        fn token(&self) -> String {
                            self.custom_token.clone()
                        }


                        fn scope(&self) -> ulo::ProviderScope {
                            self.inner_provider.scope()
                        }

                        async fn resolve(
                            &self,
                            ctx: ulo::ProviderContext,
                        ) -> Box<dyn std::any::Any + Send> {
                            self.inner_provider.resolve(ctx).await
                        }
                    }

                    let provider = std::sync::Arc::new(Box::new(CustomTokenProvider {
                        custom_token: #token_expr,
                        inner_provider,
                    }) as Box<dyn ulo::spi::Provider>);

                    ulo::spi::Injectable::new(provider, roles)
                }
            }

            #wrapper_factory_name
        }
    };

    Ok(expanded)
}
