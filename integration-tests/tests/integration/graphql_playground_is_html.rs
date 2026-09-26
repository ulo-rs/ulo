//! The GraphQL playground is served as `text/html`, on both integrations.
//!
//! The playground is a page a browser renders. Served as `text/plain`, a browser displays its
//! source as text.

use ulo::UloFactory;
use ulo::module;

use crate::common::TestServer;

mod juniper_side {
    use juniper::{EmptyMutation, EmptySubscription, RootNode, graphql_object};
    use ulo_graphql_juniper::{DefaultContext, DefaultContextBuilder, GraphQLModule};

    pub struct Query;

    #[graphql_object(context = DefaultContext)]
    impl Query {
        fn ping() -> &'static str {
            "pong"
        }
    }

    type Schema =
        RootNode<'static, Query, EmptyMutation<DefaultContext>, EmptySubscription<DefaultContext>>;

    pub fn graphql() -> GraphQLModule<
        Query,
        EmptyMutation<DefaultContext>,
        EmptySubscription<DefaultContext>,
        DefaultContextBuilder,
    > {
        let schema = Schema::new(Query, EmptyMutation::new(), EmptySubscription::new());
        GraphQLModule::for_root(schema, DefaultContextBuilder).with_playground(true)
    }
}

mod async_graphql_side {
    use ulo_graphql_async_graphql::async_graphql::{
        EmptyMutation, EmptySubscription, Object, Schema,
    };
    use ulo_graphql_async_graphql::{DefaultContextBuilder, GraphQLModule};

    #[derive(Clone)]
    pub struct Query;

    #[Object]
    impl Query {
        async fn ping(&self) -> &'static str {
            "pong"
        }
    }

    pub fn graphql() -> GraphQLModule<Query, EmptyMutation, EmptySubscription, DefaultContextBuilder>
    {
        let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
        GraphQLModule::for_root(schema, DefaultContextBuilder).with_playground(true)
    }
}

#[module(imports: [juniper_side::graphql()])]
impl JuniperModule {}

#[module(imports: [async_graphql_side::graphql()])]
impl AsyncGraphqlModule {}

async fn case_the_playground_is_html(module: impl ulo::di::ModuleMetadata + 'static) {
    let server = TestServer::start_with(UloFactory::new(), module).await;
    let resp = server
        .client()
        .get(server.url("/graphql"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("the playground answers with a content type")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        content_type.starts_with("text/html"),
        "the playground was served as {content_type}, which a browser does not render as a page"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("<html"),
        "the body is not the playground page"
    );
}

#[tokio::test]
async fn juniper_serves_the_playground_as_html() {
    case_the_playground_is_html(JuniperModule).await;
}

#[tokio::test]
async fn async_graphql_serves_the_playground_as_html() {
    case_the_playground_is_html(AsyncGraphqlModule).await;
}
