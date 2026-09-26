//! Turning an enhancer declaration into the entries a dispatch target serves with.
//!
//! A declaration names its enhancers three ways (two for an error handler), and all arrive here in
//! the order written.
//! `#[use_guards(MyGuard)]` gives a token that resolves against the role registry, so the enhancer
//! may hold injected dependencies; `#[use_guards(MyGuard{})]` gives a value built where it was
//! written; `#[use_guards(|ctx| ..)]` gives a constructor the framework runs once per execution.
//! A token that resolves against nothing fails `create`, which is what turns a misspelling into a
//! startup refusal rather than a target serving without the guard it declared.
//!
//! One copy, parameterised by the transport. What differs between them is the noun in the
//! diagnostic.

mod grpc;
mod rpc;
mod ws;

pub(crate) use self::grpc::GrpcServiceResolver;
pub(crate) use self::rpc::RpcControllerResolver;
pub(crate) use self::ws::GatewayResolver;

use crate::dispatch::transport::{
    EnhancerRegistry, EnhancerSet, GuardEntry, InterceptorEntry, Transport,
};
use crate::enhancer::{ErrorHandlerDeclaration, GuardDeclaration, InterceptorDeclaration};
use crate::error::SetupResult;

/// What one declaration names, before any of it is resolved: each role in the order written.
pub(crate) struct Declared<T: Transport> {
    pub guards: Vec<GuardDeclaration<T>>,
    pub interceptors: Vec<InterceptorDeclaration<T>>,
    pub error_handlers: Vec<ErrorHandlerDeclaration<T>>,
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

/// Every entry in the order it was written: a token looked up in the registry, a value as the
/// shared entry it already is, a constructor as the per-execution arm.
fn extend_with<T: Transport>(
    registry: &EnhancerRegistry<T>,
    set: &mut EnhancerSet<T>,
    declared: Declared<T>,
) -> SetupResult {
    for guard in declared.guards {
        set.guards.push(match guard {
            GuardDeclaration::Token(token) => registry
                .guards
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>("Guard", &token, "Guard<Context>"))?,
            GuardDeclaration::Value(guard) => GuardEntry::Ready(guard),
            GuardDeclaration::Constructor(build) => GuardEntry::Factory(build.0),
        });
    }

    for interceptor in declared.interceptors {
        set.interceptors.push(match interceptor {
            InterceptorDeclaration::Token(token) => registry
                .interceptors
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>("Interceptor", &token, "Interceptor<Context>"))?,
            InterceptorDeclaration::Value(interceptor) => InterceptorEntry::Ready(interceptor),
            InterceptorDeclaration::Constructor(build) => InterceptorEntry::Factory(build.0),
        });
    }

    for handler in declared.error_handlers {
        set.error_handlers.push(match handler {
            ErrorHandlerDeclaration::Token(token) => registry
                .error_handlers
                .get(&token)
                .cloned()
                .ok_or_else(|| {
                not_found::<T>("ErrorHandler", &token, "ErrorHandler<Context, Answer>")
            })?,
            ErrorHandlerDeclaration::Value(handler) => handler,
        });
    }

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
