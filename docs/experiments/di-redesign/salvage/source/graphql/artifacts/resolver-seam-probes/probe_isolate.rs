//! PROBE round 6 (scratch): round 4 said execute_stream loses the source,
//! round 5 said it keeps it. The only structural difference was whether the
//! schema had a Subscription root. Isolate that.

use async_graphql::dynamic::{
    Field, FieldFuture, Object, Schema, SchemaBuilder, Subscription, SubscriptionField,
    SubscriptionFieldFuture, TypeRef,
};
use async_graphql::{Request, Value};
use futures::StreamExt;

#[derive(Debug, ulo::Error)]
#[error_kind(NotFound)]
pub struct Boom;
impl std::error::Error for Boom {}
impl std::fmt::Display for Boom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "boom")
    }
}

fn query_obj() -> Object {
    Object::new("Query").field(Field::new(
        "boom",
        TypeRef::named_nn(TypeRef::STRING),
        |_| {
            FieldFuture::new(async {
                Err::<Option<Value>, _>(async_graphql::Error::new_with_source(Boom))
            })
        },
    ))
}

async fn run(label: &str, schema: Schema) {
    let r = schema.execute(Request::new("{ boom }")).await;
    println!(
        "{label:<26} execute        recovered={}",
        r.errors.first().and_then(|e| e.source::<Boom>()).is_some()
    );
    let mut s = schema.execute_stream(Request::new("{ boom }"));
    while let Some(r) = s.next().await {
        println!(
            "{label:<26} execute_stream recovered={} wire={}",
            r.errors.first().and_then(|e| e.source::<Boom>()).is_some(),
            serde_json::to_string(&r).unwrap()
        );
    }
}

#[tokio::test]
async fn subscription_root_presence() {
    // A: no subscription root — round 4's shape
    let a: SchemaBuilder = Schema::build("Query", None, None);
    run("no Subscription root", a.register(query_obj()).finish().unwrap()).await;

    // B: with a subscription root — round 5's shape
    let sub = Subscription::new("Subscription").field(SubscriptionField::new(
        "tick",
        TypeRef::named_nn(TypeRef::STRING),
        |_| {
            SubscriptionFieldFuture::new(async {
                Ok(futures::stream::iter(vec![Ok(Value::from("t"))]))
            })
        },
    ));
    let b = Schema::build("Query", None, Some("Subscription"))
        .register(query_obj())
        .register(sub)
        .finish()
        .unwrap();
    run("with Subscription root", b).await;
}
