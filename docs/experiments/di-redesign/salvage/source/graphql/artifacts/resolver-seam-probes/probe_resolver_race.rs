//! PROBE round 3 (scratch): sibling fields resolve concurrently in async-graphql.
//! Does ONE execution-scoped provider survive N concurrent resolutions, or is
//! the execution cache raced into building it more than once?

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_graphql::dynamic::{Field, FieldFuture, Object, Schema, TypeRef};
use async_graphql::{Request, Value};
use ulo::UloFactory;
use ulo::di::Execution;
use ulo::{UloApplicationContext, injectable, module};

static BUILDS: AtomicU64 = AtomicU64::new(0);
const FIELDS: usize = 24;

#[injectable(scope = "execution")]
#[derive(Clone)]
pub struct Scoped {
    #[default(0)]
    serial: u64,
}

impl Scoped {
    #[ulo::new]
    pub fn new() -> Self {
        Self { serial: BUILDS.fetch_add(1, Ordering::SeqCst) }
    }
}

#[module(providers: [Scoped])]
impl AppModule {}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_siblings_share_one_execution_scoped_instance() {
    let app = Arc::new(
        UloFactory::create_application_context(AppModule).await.expect("container"),
    );

    let names: Vec<String> = (0..FIELDS).map(|i| format!("f{i}")).collect();
    let mut query = Object::new("Query");
    for name in &names {
        let app = app.clone();
        let leaked: &'static str = Box::leak(name.clone().into_boxed_str());
        query = query.field(Field::new(leaked, TypeRef::named_nn(TypeRef::INT), move |rctx| {
            let app = app.clone();
            FieldFuture::new(async move {
                let execution = rctx.data::<Execution>()?;
                // a yield point before the resolve, to widen any race window
                tokio::task::yield_now().await;
                let s: Scoped = app.resolve::<Scoped>(execution).await
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?;
                tokio::task::yield_now().await;
                Ok(Some(Value::from(s.serial as i64)))
            })
        }));
    }

    let schema = Schema::build("Query", None, None).register(query).finish().expect("schema");
    let doc = format!("{{ {} }}", names.join(" "));

    for round in 0..8u64 {
        let before = BUILDS.load(Ordering::SeqCst);
        let mut req = Request::new(&doc);
        req.data.insert(Execution::standalone());
        let resp = schema.execute(req).await;
        let built = BUILDS.load(Ordering::SeqCst) - before;

        let v = serde_json::to_value(&resp).unwrap();
        let serials: std::collections::BTreeSet<i64> = v["data"].as_object().unwrap()
            .values().map(|x| x.as_i64().unwrap()).collect();
        println!(
            "round {round}: {FIELDS} concurrent fields -> {} distinct serial(s) {:?}, {built} build(s)",
            serials.len(), serials
        );
        assert_eq!(built, 1, "an execution-scoped provider was built more than once in one execution");
        assert_eq!(serials.len(), 1, "sibling fields saw different instances");
    }
    println!("PROBE: execution scope holds under concurrent sibling resolution");
}
