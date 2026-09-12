use std::{any::Any, future::Future, marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use ulo::{
    FxHashMap,
    di::ProviderContext,
    spi::{Provider, ProviderFactory},
};

pub(crate) struct PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    pub connect: F,
    // Injection token for this client: the `C` type name for the default (`for_root`), or the
    // caller's chosen name for a `for_root_named` client.
    pub token: String,
    pub _client: PhantomData<C>,
}

#[async_trait]
impl<C, F, Fut> ProviderFactory for PrismaClientFactory<C, F, Fut>
where
    C: Send + Sync + Clone + 'static,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = C> + Send + 'static,
{
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn build(&self, _deps: FxHashMap<String, ulo::spi::Injectable>) -> ulo::spi::Injectable {
        let client = (self.connect)().await;
        ulo::spi::Injectable::new(
            Arc::new(Box::new(PrismaClientProvider {
                client,
                token: self.token.clone(),
            })),
            vec![],
        )
    }
}

struct PrismaClientProvider<C> {
    client: C,
    token: String,
}

#[async_trait]
impl<C: Send + Sync + Clone + 'static> Provider for PrismaClientProvider<C> {
    fn token(&self) -> String {
        self.token.clone()
    }

    async fn resolve(&self, _ctx: ProviderContext) -> Box<dyn Any + Send> {
        Box::new(self.client.clone())
    }
}
