use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_graphql::{Context, EmptySubscription, Object, Schema};
use ulo::{App, AppBuilder, Dep, injectable, module};
use ulo_graphql::Engine;
use ulo_graphql_async_graphql::{AsyncGraphql, dep};
use ulo_graphql_http::{GraphqlConfig, GraphqlModule};

/// Where notes live, and the counter that numbers executions.
#[injectable]
pub struct Notes {
    #[injectable(default)]
    texts: Mutex<Vec<String>>,
    #[injectable(default)]
    executions: AtomicU64,
}

/// Built once per GraphQL execution, so every field of one request reads the same `id`.
pub struct Gql {
    pub id: u64,
}

#[injectable(execution)]
impl Gql {
    fn new(notes: Dep<Notes>) -> Self {
        Gql { id: notes.executions.fetch_add(1, Ordering::SeqCst) + 1 }
    }
}

pub struct Query;

#[Object]
impl Query {
    async fn notes(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<String>> {
        Ok(dep::<Notes>(ctx).await?.texts.lock().expect("the notes lock").clone())
    }

    async fn greeting(&self, name: String) -> String {
        format!("hello {name}")
    }

    /// The execution this field ran in.
    async fn execution(&self, ctx: &Context<'_>) -> async_graphql::Result<u64> {
        Ok(ctx.data::<Dep<Gql>>()?.id)
    }

    /// Whether a lookup of `Gql` in the request's execution finds the instance the engine built.
    async fn same_context(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(dep::<Gql>(ctx).await?.id == ctx.data::<Dep<Gql>>()?.id)
    }
}

pub struct Mutation;

#[Object]
impl Mutation {
    /// Stores `text` and answers how many notes there are now.
    async fn add_note(&self, ctx: &Context<'_>, text: String) -> async_graphql::Result<usize> {
        let notes = dep::<Notes>(ctx).await?;
        let mut texts = notes.texts.lock().expect("the notes lock");
        texts.push(text);
        Ok(texts.len())
    }
}

pub type Api = Schema<Query, Mutation, EmptySubscription>;

/// `Notes` is exported because a resolver's `dep::<Notes>` resolves with the visibility of the module
/// serving the endpoint, not of the module that binds the engine.
#[module(
    providers = [
        Notes,
        Gql,
        with = || -> Api { Schema::new(Query, Mutation, EmptySubscription) },
        AsyncGraphql<Api, Gql> as dyn Engine,
    ],
    exports = [dyn Engine, Notes, Gql],
)]
pub struct ApiModule;

#[module(imports = [GraphqlModule::for_root(GraphqlConfig::at("/graphql").playground(false).engine_from(ApiModule))])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
