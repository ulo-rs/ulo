use crate::context::StandaloneContext;
use crate::grpc::GrpcContext;
use crate::http::HttpContext;
use crate::rpc::RpcContext;
use crate::ws::WsContext;

/// The execution a provider is being built for.
///
/// Passed to [`Provider::resolve`](crate::spi::Provider::resolve) so a
/// execution-scoped provider can reach the execution it belongs to — its cache,
/// its extension bag, and whatever the transport carries.
///
/// Each variant holds a context handle, which is cheap to clone. State every
/// execution has is reached through
/// [`HandlerContext`](crate::context::HandlerContext) whichever variant this is;
/// state one transport has is reached by matching, and a standalone execution has
/// none to match on.
#[derive(Clone)]
#[non_exhaustive]
pub enum ProviderContext {
    Http(HttpContext),
    Ws(WsContext),
    Rpc(RpcContext),
    Grpc(GrpcContext),
    /// An execution with no transport behind it — a CLI command, a job, a test.
    Standalone(StandaloneContext),
    /// No active execution — module initialisation, `ApplicationContext::get`.
    None,
}

impl ProviderContext {
    /// A fresh execution belonging to no transport.
    ///
    /// What a caller resolving providers by hand starts with. Everything resolved
    /// against the returned value shares one cache, so an execution-scoped provider is
    /// built once for all of them; the execution ends when the value is dropped.
    pub fn standalone() -> Self {
        Self::Standalone(StandaloneContext::new())
    }

    /// The execution's instance cache, or `None` outside an execution.
    ///
    /// This is what an execution-scoped provider needs and the only thing it needs
    /// from every transport, which is why it is reachable without matching.
    pub fn cache(&self) -> Option<&crate::di::ExecutionCache> {
        use crate::context::HandlerContext;
        match self {
            Self::Http(c) => Some(c.cache()),
            Self::Ws(c) => Some(c.cache()),
            Self::Rpc(c) => Some(c.cache()),
            Self::Grpc(c) => Some(c.cache()),
            Self::Standalone(c) => Some(c.cache()),
            Self::None => None,
        }
    }

    /// The execution's extension bag, or `None` outside an execution.
    pub fn extensions(&self) -> Option<crate::context::Extensions> {
        use crate::context::HandlerContext;
        match self {
            Self::Http(c) => Some(c.extensions().clone()),
            Self::Ws(c) => Some(c.extensions().clone()),
            Self::Rpc(c) => Some(c.extensions().clone()),
            Self::Grpc(c) => Some(c.extensions().clone()),
            Self::Standalone(c) => Some(c.extensions().clone()),
            Self::None => None,
        }
    }

    /// Refuses a provider whose scope this execution cannot satisfy.
    ///
    /// An execution-scoped instance lives in the execution's cache. Where there is no
    /// execution there is nowhere to put it, and the generated provider panics on
    /// the missing cache — so a caller resolving by hand checks here first and
    /// returns the refusal instead.
    pub(crate) fn ensure_can_build(
        &self,
        scope: crate::di::ProviderScope,
        token: &str,
    ) -> Result<(), crate::error::ResolutionError> {
        if scope == crate::di::ProviderScope::Execution && self.cache().is_none() {
            return Err(crate::error::ResolutionError::ExecutionRequired {
                token: token.to_string(),
            });
        }

        Ok(())
    }

    /// The HTTP request parts, when this execution is an HTTP one.
    pub fn request_parts(&self) -> Option<&crate::http::RequestPart> {
        match self {
            Self::Http(c) => Some(c.request()),
            _ => None,
        }
    }
}

impl From<HttpContext> for ProviderContext {
    fn from(ctx: HttpContext) -> Self {
        Self::Http(ctx)
    }
}

impl From<WsContext> for ProviderContext {
    fn from(ctx: WsContext) -> Self {
        Self::Ws(ctx)
    }
}

impl From<RpcContext> for ProviderContext {
    fn from(ctx: RpcContext) -> Self {
        Self::Rpc(ctx)
    }
}

impl From<GrpcContext> for ProviderContext {
    fn from(ctx: GrpcContext) -> Self {
        Self::Grpc(ctx)
    }
}

impl From<StandaloneContext> for ProviderContext {
    fn from(ctx: StandaloneContext) -> Self {
        Self::Standalone(ctx)
    }
}
