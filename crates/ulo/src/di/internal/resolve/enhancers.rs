//! Turning an enhancer declaration into the entries a dispatch target serves with.
//!
//! A declaration names its enhancers two ways, and both arrive here. `#[use_guards(MyGuard)]` gives
//! a token that resolves against the role registry, so the enhancer may hold injected dependencies;
//! `#[use_guards(MyGuard{})]` gives a value built where it was written. A token that resolves
//! against nothing fails `create`, which is what turns a misspelling into a startup refusal rather
//! than a target serving without the guard it declared.
//!
//! One copy, parameterised by the transport. What differs between them is the noun in the
//! diagnostic.

use std::sync::Arc;

use crate::enhancer::{Guard, Interceptor};
use crate::error::SetupResult;
use crate::spi::transport::{
    EnhancerRegistry, EnhancerSet, ErrorHandlerArc, GuardEntry, InterceptorEntry, Transport,
};

/// What one declaration names, before any of it is resolved.
pub(crate) struct Declared<T: Transport> {
    pub guard_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<T::Context>>>,
    pub interceptor_tokens: Vec<String>,
    pub interceptors: Vec<Arc<dyn Interceptor<T::Context, T::Answer>>>,
    pub error_handler_tokens: Vec<String>,
    pub error_handlers: Vec<ErrorHandlerArc<T>>,
}

/// Resolve what a dispatch target declares, with the transport's globals ahead of it.
///
/// The globals belong to this tier alone. A handler's own entries stack on what is resolved here,
/// so resolving those with the globals too would run each global twice.
pub(crate) fn resolve_target<T: Transport>(
    registry: &EnhancerRegistry<T>,
    globals: &EnhancerSet<T>,
    declared: Declared<T>,
) -> SetupResult<EnhancerSet<T>> {
    let mut set = globals.clone();
    extend_with(registry, &mut set, declared)?;
    Ok(set)
}

/// Resolve what one handler declares on top of its target's. No globals: they are already in the
/// target-level set this stacks on.
pub(crate) fn resolve_handler<T: Transport>(
    registry: &EnhancerRegistry<T>,
    declared: Declared<T>,
) -> SetupResult<EnhancerSet<T>> {
    let mut set = EnhancerSet::default();
    extend_with(registry, &mut set, declared)?;
    Ok(set)
}

/// DI-resolved entries first, then the ones built at the declaration site.
fn extend_with<T: Transport>(
    registry: &EnhancerRegistry<T>,
    set: &mut EnhancerSet<T>,
    declared: Declared<T>,
) -> SetupResult {
    for token in declared.guard_tokens {
        set.guards.push(
            registry
                .guards
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>("Guard", &token, "Guard<Context>"))?,
        );
    }
    set.guards
        .extend(declared.guards.into_iter().map(GuardEntry::Ready));

    for token in declared.interceptor_tokens {
        set.interceptors.push(
            registry
                .interceptors
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>("Interceptor", &token, "Interceptor<Context>"))?,
        );
    }
    set.interceptors.extend(
        declared
            .interceptors
            .into_iter()
            .map(InterceptorEntry::Ready),
    );

    for token in declared.error_handler_tokens {
        set.error_handlers.push(
            registry
                .error_handlers
                .get(&token)
                .cloned()
                .ok_or_else(|| {
                    not_found::<T>("ErrorHandler", &token, "ErrorHandler<Context, Answer>")
                })?,
        );
    }
    set.error_handlers.extend(declared.error_handlers);

    Ok(())
}

/// The one diagnostic a token that resolves against nothing produces.
///
/// A role is registered by the trait impl a provider carries, so a token missing from the registry
/// means either the provider is absent from `providers` or it does not implement the role at all.
/// Both are worth naming, because the second compiles.
fn not_found<T: Transport>(
    role: &str,
    token: &str,
    trait_shape: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "{transport} {role} '{token}' not found in registry. A {lower} registers automatically by \
         implementing {trait_shape} for this transport's context; make sure the provider is in the \
         module's `providers` list. For `provider_factory!` under a string/const token, name the \
         produced type so it can be detected — annotate the closure's return type or pass a type \
         hint.",
        transport = T::NAME,
        lower = role.to_lowercase(),
    )
    .into()
}
