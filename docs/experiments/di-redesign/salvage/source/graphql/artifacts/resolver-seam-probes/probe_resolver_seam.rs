//! PROBE (scratch, not a deliverable): can an `async_graphql::dynamic` resolver
//! closure enter ulo's pipeline — DI-resolve a provider at the request's own
//! execution, run a guard, extract an argument, call the handler?
//!
//! The falsifier: `Field::new` wants
//! `for<'a> Fn(ResolverContext<'a>) -> FieldFuture<'a> + Send + Sync + 'static`,
//! built once at schema-build time, while the execution is per-request. The
//! mechanism under test is carrying ulo's `Execution` through async-graphql's
//! request `Data` and reading it back inside the closure.

use std::sync::Arc;

use async_graphql::dynamic::{Field, FieldFuture, InputValue, Object, Schema, TypeRef};
use async_graphql::{Request, Value};
use async_trait::async_trait;
use ulo::UloFactory;
use ulo::context::{ExecutionContext, StandaloneContext};
use ulo::di::Execution;
use ulo::enhancer::Guard;
use ulo::{injectable, module};

// ---- an ordinary ulo provider graph -----------------------------------

#[injectable]
#[derive(Clone)]
pub struct HeroRepo {}

impl HeroRepo {
    fn name_of(&self, id: i64) -> String {
        format!("hero-{id}")
    }
}

#[injectable]
#[derive(Clone)]
pub struct HeroService {
    #[inject]
    repo: HeroRepo,
}

impl HeroService {
    fn find(&self, id: i64) -> String {
        self.repo.name_of(id)
    }
}

// ---- an ordinary ulo guard --------------------------------------------

struct AuthGuard;

#[async_trait]
impl Guard<StandaloneContext> for AuthGuard {
    async fn can_activate(&self, ctx: &StandaloneContext) -> bool {
        // Reads the execution's own extension bag, as a real guard would.
        ctx.extensions().get::<bool>().unwrap_or(false)
    }
}

#[module(providers: [HeroRepo, HeroService])]
impl AppModule {}

#[tokio::test]
async fn a_dynamic_resolver_closure_enters_the_ulo_pipeline() {
    let app_ctx = Arc::new(
        UloFactory::create_application_context(AppModule)
            .await
            .expect("container built"),
    );

    // Built ONCE, at schema-build time. Captures the container, not a request.
    let resolver_ctx = app_ctx.clone();
    let hero = Field::new(
        "hero",
        TypeRef::named_nn(TypeRef::STRING),
        move |rctx| {
            let app = resolver_ctx.clone();
            FieldFuture::new(async move {
                // 1. find THIS request's execution
                let execution = rctx.data::<Execution>()?;

                // 2. run a ulo guard against it
                let standalone = match execution {
                    Execution::Standalone(s) => s.clone(),
                    _ => return Err("probe expects a standalone execution".into()),
                };
                if !AuthGuard.can_activate(&standalone).await {
                    return Err("Forbidden".into());
                }

                // 3. DI-resolve at that execution
                let svc: HeroService = app
                    .resolve::<HeroService>(execution)
                    .await
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?;

                // 4. extract the argument, call the handler
                let id = rctx.args.try_get("id")?.i64()?;
                Ok(Some(Value::from(svc.find(id))))
            })
        },
    )
    .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::INT)));

    let query = Object::new("Query").field(hero);
    let schema = Schema::build("Query", None, None)
        .register(query)
        .finish()
        .expect("schema builds");

    // --- allowed ---
    let execution = Execution::standalone();
    if let Execution::Standalone(s) = &execution {
        s.extensions().insert(true);
    }
    let mut req = Request::new("{ hero(id: 7) }");
    req.data.insert(execution);
    let resp = schema.execute(req).await;
    println!("PROBE allowed  -> {}", serde_json::to_string(&resp).unwrap());

    // --- guard refuses ---
    let mut req = Request::new("{ hero(id: 7) }");
    req.data.insert(Execution::standalone());
    let resp = schema.execute(req).await;
    println!("PROBE refused  -> {}", serde_json::to_string(&resp).unwrap());
}
