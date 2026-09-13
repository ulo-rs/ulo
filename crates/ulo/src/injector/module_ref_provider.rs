use std::{any::Any, sync::Arc};

use parking_lot::RwLock;

use crate::async_trait;
use crate::di::{ProviderContext, ProviderScope};
use crate::spi::Provider;

use super::{ModuleRef, module_ref::ProviderStore};

pub(crate) struct ModuleRefProvider {
    module_token: String,
    store: Arc<RwLock<ProviderStore>>,
}

impl ModuleRefProvider {
    pub(crate) fn new(module_token: String, store: Arc<RwLock<ProviderStore>>) -> Self {
        Self {
            module_token,
            store,
        }
    }
}

#[async_trait]
impl Provider for ModuleRefProvider {
    async fn resolve(&self, _ctx: ProviderContext) -> Box<dyn Any + Send> {
        Box::new(ModuleRef::new(
            self.module_token.clone(),
            self.store.clone(),
        ))
    }

    fn token(&self) -> String {
        crate::di::token_of::<ModuleRef>()
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }
}
