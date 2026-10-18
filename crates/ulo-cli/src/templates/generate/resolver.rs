use async_graphql::Object;

/// A GraphQL root object. Merge it into the query root with `#[derive(MergedObject, Default)]`,
/// or use it as the root itself.
#[derive(Default)]
pub struct @@pascal@@Resolver;

#[Object]
impl @@pascal@@Resolver {
    #[graphql(name = "@@field@@")]
    async fn @@snake@@(&self) -> &'static str {
        "@@snake@@"
    }
}

#[cfg(test)]
mod tests {
    use async_graphql::{EmptyMutation, EmptySubscription, Schema};

    use super::@@pascal@@Resolver;

    #[tokio::test]
    async fn answers_its_field() {
        let schema = Schema::new(@@pascal@@Resolver, EmptyMutation, EmptySubscription);

        let response = schema.execute("{ @@field@@ }").await;

        assert!(response.errors.is_empty(), "{:?}", response.errors);
    }
}
