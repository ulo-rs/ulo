use crate::di::ModuleMetadata;
use crate::spi::{ControllerFactory, ProviderFactory};
use crate::ws::BroadcastService;
use crate::ws::broadcast_provider::BroadcastServiceManager;

/// Opt-in module that provides `BroadcastService` for WebSocket broadcasting.
///
/// Import this in any module whose gateways need `BroadcastService`. Because the
/// module is global, any module that transitively imports it can inject the service
/// without re-exporting it.
///
/// # Example
///
/// ```rust,ignore
/// #[module(
///     imports: [BroadcastModule::new()],
///     providers: [ChatGateway],
/// )]
/// struct AppModule;
/// ```
pub struct BroadcastModule;

impl BroadcastModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleMetadata for BroadcastModule {
    fn identity(&self) -> crate::di::ModuleIdentity {
        crate::di::ModuleIdentity::named("UloBroadcastModule")
    }

    fn is_global(&self) -> bool {
        true
    }

    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        None
    }

    fn controllers(&self) -> Option<Vec<Box<dyn ControllerFactory>>> {
        None
    }

    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> {
        Some(vec![Box::new(BroadcastServiceManager)])
    }

    fn exports(&self) -> Option<Vec<String>> {
        Some(vec![crate::di::token_of::<BroadcastService>()])
    }
}
