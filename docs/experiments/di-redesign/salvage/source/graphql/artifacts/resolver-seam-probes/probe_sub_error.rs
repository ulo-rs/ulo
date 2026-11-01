//! PROBE round 5 (scratch): where exactly does the typed error survive on the
//! streaming path — subscription setup failure vs mid-stream item failure vs a
//! query pushed through execute_stream.

use async_graphql::dynamic::{
    Field, FieldFuture, Object, Schema, Subscription, SubscriptionField,
    SubscriptionFieldFuture, TypeRef,
};
use async_graphql::{Request, Value};
use futures::StreamExt;

#[derive(Debug, ulo::Error)]
#[error_kind(NotFound)]
pub enum ShopError {
    Nope,
}
impl std::error::Error for ShopError {}
impl std::fmt::Display for ShopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "nope")
    }
}

fn err() -> async_graphql::Error {
    async_graphql::Error::new_with_source(ShopError::Nope)
}

#[tokio::test]
async fn where_the_source_survives() {
    let query = Object::new("Query")
        .field(Field::new("ok", TypeRef::named_nn(TypeRef::STRING), |_| {
            FieldFuture::new(async { Ok(Some(Value::from("fine"))) })
        }))
        .field(Field::new("boom", TypeRef::named_nn(TypeRef::STRING), |_| {
            FieldFuture::new(async { Err::<Option<Value>, _>(err()) })
        }));

    let sub = Subscription::new("Subscription")
        // (a) setup fails before any item
        .field(SubscriptionField::new(
            "failsAtSetup",
            TypeRef::named_nn(TypeRef::STRING),
            |_| {
                SubscriptionFieldFuture::new(async {
                    Err::<futures::stream::Empty<Result<Value, async_graphql::Error>>, _>(err())
                })
            },
        ))
        // (b) one good item, then a failing item
        .field(SubscriptionField::new(
            "failsMidStream",
            TypeRef::named_nn(TypeRef::STRING),
            |_| {
                SubscriptionFieldFuture::new(async {
                    Ok(futures::stream::iter(vec![
                        Ok(Value::from("first")),
                        Err(err()),
                    ]))
                })
            },
        ));

    let schema = Schema::build("Query", None, Some("Subscription"))
        .register(query)
        .register(sub)
        .finish()
        .expect("schema");

    let show = |label: &str, r: &async_graphql::Response| {
        println!(
            "{label:<34} recovered={:<5} wire={}",
            r.errors.first().and_then(|e| e.source::<ShopError>()).is_some(),
            serde_json::to_string(r).unwrap()
        );
    };

    // baseline: plain execute
    show("execute (query)", &schema.execute(Request::new("{ boom }")).await);

    // query pushed through execute_stream
    let mut s = schema.execute_stream(Request::new("{ boom }"));
    while let Some(r) = s.next().await {
        show("execute_stream (query)", &r);
    }

    // (a) subscription setup failure
    let mut s = schema.execute_stream(Request::new("subscription { failsAtSetup }"));
    while let Some(r) = s.next().await {
        show("subscription setup failure", &r);
    }

    // (b) subscription mid-stream item failure
    let mut s = schema.execute_stream(Request::new("subscription { failsMidStream }"));
    while let Some(r) = s.next().await {
        show("subscription mid-stream", &r);
    }

    // (c) a validation failure on the streaming path (no source to begin with)
    let mut s = schema.execute_stream(Request::new("subscription { nosuchfield }"));
    while let Some(r) = s.next().await {
        show("subscription validation error", &r);
    }
}
