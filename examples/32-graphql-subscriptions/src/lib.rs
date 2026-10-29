use async_graphql::{Context, EmptyMutation, Object, Schema, Subscription};
use futures_util::{Stream, stream};
use tokio::sync::broadcast;
use ulo::{App, AppBuilder, injectable, module};
use ulo_graphql::Engine;
use ulo_graphql_async_graphql::{AsyncGraphql, dep};
use ulo_graphql_http::{GraphqlConfig, GraphqlModule};
use ulo_ws::WsModule;

/// What feeds the subscription: whatever calls `publish` reaches every subscriber at that moment.
pub struct Ticker {
    sender: broadcast::Sender<u64>,
}

#[injectable]
impl Ticker {
    fn new() -> Self {
        Ticker { sender: broadcast::channel(16).0 }
    }
}

impl Ticker {
    pub fn publish(&self, tick: u64) {
        let _ = self.sender.send(tick);
    }

    /// How many subscriptions are listening.
    pub fn listeners(&self) -> usize {
        self.sender.receiver_count()
    }
}

/// The engine needs a context type; this one carries nothing.
pub struct Gql;

#[injectable(execution)]
impl Gql {
    fn new() -> Self {
        Gql
    }
}

pub struct Query;

#[Object]
impl Query {
    async fn status(&self) -> &'static str {
        "up"
    }
}

pub struct Subscriptions;

#[Subscription]
impl Subscriptions {
    /// Every tick published while the subscription is open.
    async fn ticks(&self, ctx: &Context<'_>) -> async_graphql::Result<impl Stream<Item = u64>> {
        let receiver = dep::<Ticker>(ctx).await?.sender.subscribe();
        Ok(stream::unfold(receiver, |mut receiver| async { receiver.recv().await.ok().map(|tick| (tick, receiver)) }))
    }
}

pub type Api = Schema<Query, EmptyMutation, Subscriptions>;

#[module(
    providers = [
        Ticker,
        Gql,
        with = || -> Api { Schema::new(Query, EmptyMutation, Subscriptions) },
        AsyncGraphql<Api, Gql> as dyn Engine,
    ],
    exports = [dyn Engine, Ticker],
)]
pub struct ApiModule;

#[module(imports = [
    WsModule::for_root(),
    ApiModule,
    GraphqlModule::for_root(GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").playground(false).engine_from(ApiModule)),
])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
