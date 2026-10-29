use std::sync::Mutex;

use ulo::{AnyGuard, AnyInterceptor, App, AppBuilder, BoxError, Dep, ErrorHandler, Guard, GuardRejected, Interceptor, Next, injectable, module, routes};
use ulo_http::{HttpBody, HttpCx, Http, Response, StatusCode};

/// What the pipeline did, in order, so a test can read it back.
#[injectable]
pub struct Trace {
    #[injectable(default)]
    lines: Mutex<Vec<String>>,
}

impl Trace {
    pub fn push(&self, line: impl Into<String>) {
        self.lines.lock().unwrap().push(line.into());
    }

    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.lines.lock().unwrap())
    }
}

/// Declared on a handler with `#[meta(..)]`; a guard reads it through the handler's info.
pub struct RequiresRole(pub &'static str);

/// Writes a line when it runs as a guard and a line on each side of the handler as an interceptor.
pub struct Stamp {
    tier: &'static str,
    trace: Dep<Trace>,
}

impl Stamp {
    fn new(tier: &'static str, trace: Dep<Trace>) -> Self {
        Stamp { tier, trace }
    }
}

impl Guard<Http> for Stamp {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        self.trace.push(format!("guard {}", self.tier));
        Ok(true)
    }
}

impl Interceptor<Http> for Stamp {
    async fn intercept(&self, _cx: &HttpCx, next: Next<'_, Http>) -> Result<Response, BoxError> {
        self.trace.push(format!("{} in", self.tier));
        let reply = next.run().await;
        self.trace.push(format!("{} out", self.tier));
        reply
    }
}

/// Admits a call whose `x-role` header names the role the handler's `RequiresRole` declares.
#[injectable]
pub struct RoleGuard {
    trace: Dep<Trace>,
}

impl Guard<Http> for RoleGuard {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        let needed = cx.exec().handler().and_then(|handler| handler.meta::<RequiresRole>()).map(|role| role.0);
        let given = cx.headers().get("x-role").and_then(|value| value.to_str().ok());
        let admitted = needed.is_none_or(|needed| given == Some(needed));
        self.trace.push(format!("guard role: {}", if admitted { "admitted" } else { "refused" }));
        Ok(admitted)
    }
}

/// Answers a guard's refusal in its own words and hands every other error on.
#[injectable]
pub struct Refusals {
    trace: Dep<Trace>,
}

impl ErrorHandler<Http> for Refusals {
    async fn handle(&self, err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        let Some(refused) = err.downcast_ref::<GuardRejected>() else { return Err(err) };
        self.trace.push("error handler");
        let needed = cx.exec().handler().and_then(|handler| handler.meta::<RequiresRole>()).map_or("-", |role| role.0);
        let mut reply = Response::new(HttpBody::from(format!("{} needs role {needed}", refused.guard.rsplit("::").next().unwrap_or(refused.guard))));
        *reply.status_mut() = StatusCode::FORBIDDEN;
        Ok(reply)
    }
}

#[injectable]
pub struct ReportsController {
    trace: Dep<Trace>,
}

#[routes]
#[guards(RoleGuard, with = |trace: Dep<Trace>| Stamp::new("controller", trace))]
#[interceptors(with = |trace: Dep<Trace>| Stamp::new("controller", trace))]
#[error_handlers(Refusals)]
impl ReportsController {
    #[ulo_http::get("/reports")]
    fn open(&self) -> &'static str {
        self.trace.push("handler");
        "open report"
    }

    #[ulo_http::get("/reports/payroll")]
    #[guards(with = |trace: Dep<Trace>| Stamp::new("method", trace))]
    #[interceptors(with = |trace: Dep<Trace>| Stamp::new("method", trace))]
    #[meta(RequiresRole("admin"))]
    fn payroll(&self) -> &'static str {
        self.trace.push("handler");
        "payroll report"
    }
}

#[module(
    providers = [
        Trace,
        RoleGuard,
        Refusals,
        into AnyGuard<Http>: [with = |trace: Dep<Trace>| Stamp::new("global", trace)],
        into AnyInterceptor<Http>: [with = |trace: Dep<Trace>| Stamp::new("global", trace)],
    ],
    controllers = [ReportsController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
