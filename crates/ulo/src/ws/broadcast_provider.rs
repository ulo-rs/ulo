use std::any::Any;
use std::sync::Arc;

use super::BroadcastService;
use crate::FxHashMap;
use crate::async_trait;
use crate::di::ProviderContext;
use crate::provider_scope::ProviderScope;
use crate::spi::{Provider, ProviderFactory};
/// Singleton provider that hands out clones of the pre-built `BroadcastService`.
pub(crate) struct BroadcastServiceProvider {
    instance: BroadcastService,
}

#[async_trait]
impl Provider for BroadcastServiceProvider {
    fn token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn resolve(&self, _ctx: ProviderContext) -> Box<dyn Any + Send> {
        Box::new(self.instance.clone())
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }
}

impl Clone for BroadcastServiceProvider {
    fn clone(&self) -> Self {
        Self {
            instance: self.instance.clone(),
        }
    }
}

pub(crate) struct BroadcastServiceManager;

#[async_trait]
impl ProviderFactory for BroadcastServiceManager {
    fn token(&self) -> String {
        crate::di::token_of::<BroadcastService>()
    }

    async fn build(
        &self,
        _deps: FxHashMap<String, crate::spi::Injectable>,
    ) -> crate::spi::Injectable {
        crate::spi::Injectable::new(
            Arc::new(Box::new(BroadcastServiceProvider {
                instance: BroadcastService::new(),
            }) as Box<dyn Provider>),
            vec![],
        )
    }
}
