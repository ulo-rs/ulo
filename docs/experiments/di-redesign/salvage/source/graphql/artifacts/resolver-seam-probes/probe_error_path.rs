//! PROBE round 4 (scratch): can a TYPED domain error travel from a dynamic
//! resolver out to something ulo could hand `#[catch(MyError)]`? And do
//! non-scalar (object/nested) returns work through the same closure seam?

use std::sync::Arc;

use async_graphql::dynamic::{Field, FieldFuture, FieldValue, Object, Schema, TypeRef};
use async_graphql::{Request, Value};
use futures::StreamExt;

// A user's domain error — implements ulo::Error, as a real one would.
#[derive(Debug, ulo::Error)]
#[error_kind(NotFound)]
pub enum ShopError {
    NoSuchHero { id: i64 },
}

impl std::error::Error for ShopError {}

impl std::fmt::Display for ShopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShopError::NoSuchHero { id } => write!(f, "no hero {id}"),
        }
    }
}

#[tokio::test]
async fn a_typed_domain_error_survives_execution() {
    // --- an object type with nested fields, returned from a closure ---
    let hero = Object::new("Hero")
        .field(Field::new("name", TypeRef::named_nn(TypeRef::STRING), |ctx| {
            FieldFuture::new(async move {
                let h = ctx.parent_value.try_downcast_ref::<(i64, String)>()?;
                Ok(Some(Value::from(h.1.clone())))
            })
        }))
        .field(Field::new("id", TypeRef::named_nn(TypeRef::INT), |ctx| {
            FieldFuture::new(async move {
                let h = ctx.parent_value.try_downcast_ref::<(i64, String)>()?;
                Ok(Some(Value::from(h.0)))
            })
        }));

    let query = Object::new("Query")
        // nested object return
        .field(Field::new("hero", TypeRef::named_nn("Hero"), |_| {
            FieldFuture::new(async move {
                Ok(Some(FieldValue::owned_any((7i64, "Ada".to_string()))))
            })
        }))
        // typed error, carried in the source slot
        .field(Field::new("boom", TypeRef::named_nn(TypeRef::STRING), |_| {
            FieldFuture::new(async move {
                Err::<Option<Value>, _>(async_graphql::Error::new_with_source(
                    ShopError::NoSuchHero { id: 9 },
                ))
            })
        }))
        // control: an error with NO source
        .field(Field::new("plain", TypeRef::named_nn(TypeRef::STRING), |_| {
            FieldFuture::new(async move {
                Err::<Option<Value>, _>(async_graphql::Error::new("just a string"))
            })
        }));

    let schema = Schema::build("Query", None, None)
        .register(hero)
        .register(query)
        .finish()
        .expect("schema builds");

    // (1) nested object
    let r = schema.execute(Request::new("{ hero { id name } }")).await;
    println!("PROBE nested object -> {}", serde_json::to_string(&r).unwrap());

    // (2) typed error round-trip
    let r = schema.execute(Request::new("{ boom }")).await;
    println!("PROBE typed error wire -> {}", serde_json::to_string(&r).unwrap());
    let recovered = r.errors.first().and_then(|e| e.source::<ShopError>());
    match recovered {
        Some(e) => println!(
            "PROBE typed error RECOVERED -> {:?}, ulo kind = {:?}",
            e,
            ulo::Error::kind(e)
        ),
        None => println!("PROBE typed error LOST"),
    }

    // (3) control — must be None, or check (2) proves nothing
    let r = schema.execute(Request::new("{ plain }")).await;
    let ctrl = r.errors.first().and_then(|e| e.source::<ShopError>());
    println!("PROBE control (no source) -> {}", if ctrl.is_none() { "None (check is sound)" } else { "SOMETHING — check is broken" });

    // (4) does it survive a subscription's stream?
    let sub_schema_err = schema.execute(Request::new("{ boom hero { id } }")).await;
    println!(
        "PROBE error beside a sibling -> {} | source recovered: {}",
        serde_json::to_string(&sub_schema_err).unwrap(),
        sub_schema_err.errors.first().and_then(|e| e.source::<ShopError>()).is_some()
    );

    // (5) through execute_stream, the path a subscription binding uses
    let mut s = schema.execute_stream(Request::new("{ boom }"));
    while let Some(r) = s.next().await {
        println!(
            "PROBE via execute_stream -> source recovered: {}",
            r.errors.first().and_then(|e| e.source::<ShopError>()).is_some()
        );
    }
}
