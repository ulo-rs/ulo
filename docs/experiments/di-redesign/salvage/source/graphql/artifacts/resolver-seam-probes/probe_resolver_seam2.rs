//! PROBE round 2 (scratch): the three things round 1 did not show.
//!  (a) do two fields in ONE document share one execution-scoped provider?
//!  (b) does a ulo Interceptor — boxed `next`, async — fit the closure?
//!  (c) does a subscription field keep the same shape and stay 'static?

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_graphql::dynamic::{
    Field, FieldFuture, Object, Schema, Subscription, SubscriptionField,
    SubscriptionFieldFuture, TypeRef,
};
use async_graphql::{Request, Value};
use async_trait::async_trait;
use futures::StreamExt;
use ulo::UloFactory;
use ulo::context::StandaloneContext;
use ulo::di::Execution;
use ulo::enhancer::{Interceptor, InterceptorNext};
use ulo::{UloApplicationContext, injectable, module};

static BUILDS: AtomicU64 = AtomicU64::new(0);

/// Execution-scoped: one per execution, shared by everything in it.
/// Each construction takes the next serial, so a shared instance is visible
/// as one serial and a rebuilt one as two.
#[injectable(scope = "execution")]
#[derive(Clone)]
pub struct RequestId {
    #[default(0)]
    serial: u64,
}

impl RequestId {
    #[ulo::new]
    pub fn new() -> Self {
        Self {
            serial: BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }

    fn id(&self) -> u64 {
        self.serial
    }
}

#[async_trait]
impl Interceptor<StandaloneContext, Result<String, String>> for Tag {
    async fn intercept(
        &self,
        ctx: &StandaloneContext,
        next: Box<dyn InterceptorNext<StandaloneContext, Result<String, String>>>,
    ) -> Result<String, String> {
        let inner = next.run(ctx).await?;
        Ok(format!("[tagged]{inner}"))
    }
}

struct Tag;

#[module(providers: [RequestId])]
impl AppModule {}

fn field(app: Arc<UloApplicationContext>, name: &'static str) -> Field {
    Field::new(name, TypeRef::named_nn(TypeRef::STRING), move |rctx| {
        let app = app.clone();
        FieldFuture::new(async move {
            let execution = rctx.data::<Execution>()?;
            let rid: RequestId = app
                .resolve::<RequestId>(execution)
                .await
                .map_err(|e| async_graphql::Error::new(e.to_string()))?;
            Ok(Some(Value::from(format!("{name}:{}", rid.id()))))
        })
    })
}

#[tokio::test]
async fn execution_scope_and_interceptor_and_subscription() {
    let app = Arc::new(
        UloFactory::create_application_context(AppModule)
            .await
            .expect("container"),
    );

    // (a) two fields, one document
    let query = Object::new("Query")
        .field(field(app.clone(), "alpha"))
        .field(field(app.clone(), "beta"));

    // (c) a subscription field, same closure shape
    let app_sub = app.clone();
    let sub = Subscription::new("Subscription").field(SubscriptionField::new(
        "ticks",
        TypeRef::named_nn(TypeRef::STRING),
        move |rctx| {
            let app = app_sub.clone();
            SubscriptionFieldFuture::new(async move {
                let execution = rctx.data::<Execution>()?;
                let rid: RequestId = app
                    .resolve::<RequestId>(execution)
                    .await
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?;
                let id = rid.id();
                Ok(futures::stream::iter(0..3)
                    .map(move |n| Ok(Value::from(format!("tick{n}@{id}")))))
            })
        },
    ));

    let schema = Schema::build("Query", None, Some("Subscription"))
        .register(query)
        .register(sub)
        .finish()
        .expect("schema builds");

    let mut req = Request::new("{ alpha beta }");
    req.data.insert(Execution::standalone());
    let resp = schema.execute(req).await;
    println!("PROBE (a) two fields, one execution -> {}", serde_json::to_string(&resp).unwrap());

    let mut req2 = Request::new("{ alpha beta }");
    req2.data.insert(Execution::standalone());
    let resp2 = schema.execute(req2).await;
    println!("PROBE (a) a SECOND execution        -> {}", serde_json::to_string(&resp2).unwrap());
    println!("PROBE (a) total RequestId builds    -> {}", BUILDS.load(Ordering::SeqCst));

    // (b) interceptor around a handler body
    let ctx = StandaloneContext::new();
    let out = Tag
        .intercept(&ctx, Box::new(Handler))
        .await;
    println!("PROBE (b) interceptor               -> {out:?}");

    // (c) drive the subscription
    let mut req3 = Request::new("subscription { ticks }");
    req3.data.insert(Execution::standalone());
    let mut stream = schema.execute_stream(req3);
    let mut got = Vec::new();
    while let Some(r) = stream.next().await {
        got.push(serde_json::to_string(&r).unwrap());
    }
    println!("PROBE (c) subscription              -> {got:?}");
}

struct Handler;

#[async_trait]
impl InterceptorNext<StandaloneContext, Result<String, String>> for Handler {
    async fn run(self: Box<Self>, _ctx: &StandaloneContext) -> Result<String, String> {
        Ok("handler-ran".into())
    }
}
