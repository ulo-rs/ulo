//! A gRPC service whose one method, `Counter.Add`, answers by its first operand: a sum, a call
//! held until the test releases it, or an error carrying a chosen set of details.
//!
//! | `a` | answer |
//! |---|---|
//! | `>= 0`, not 100 | `Sum { a + b }` |
//! | 100 | records `entered`, waits for `gate`, then `Sum { a + b }`; records its cancel reason in `dropped` as it ends |
//! | -1 | `Conflict` with an `ErrorInfo` |
//! | -2 | `BadRequest` with two `FieldViolations`, a `Help` and a `RetryAfter` |
//! | -3 | `NotFound` with a `Json` detail |
//! | -4 | `NotFound` with no details |
//! | -5 | `Conflict` sent with the gRPC code `b` |
//! | -6 | `Conflict` with two `ErrorInfo` details |

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use ulo::{CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::Message;
use ulo_transport::{CallError, Detail, Details, ErrorKind, FieldViolation, Link};

use crate::support::Record;

/// What the held call (`a = 100`) signals and waits on.
#[derive(Clone)]
pub struct Hold {
    pub entered: Record<()>,
    pub gate: Arc<Notify>,
    /// The held call's cancel reason, written as its handler's future drops.
    pub dropped: Record<Option<CancelReason>>,
}

struct OnDrop(ExecutionRef, Record<Option<CancelReason>>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.1.push(self.0.cancel_reason());
    }
}

impl Hold {
    pub fn new() -> Hold {
        Hold { entered: Record::new(), gate: Arc::new(Notify::new()), dropped: Record::new() }
    }
}

#[injectable]
pub struct Service {
    hold: Dep<Hold>,
}

#[routes]
impl Service {
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<AddRequest>, exec: ExecutionRef) -> Result<Sum, CallError> {
        let AddRequest { a, b } = req.0;
        match a {
            100 => {
                let _dropped = OnDrop(exec, self.hold.dropped.clone());
                self.hold.entered.push(());
                self.hold.gate.notified().await;
                Ok(Sum { sum: a + b })
            }
            -1 => Err(CallError::new(ErrorKind::Conflict, "taken").with_details(Details::new().with(info("NAME_TAKEN", "counter.example", &[("name", "x")])))),
            -2 => Err(CallError::new(ErrorKind::BadRequest, "invalid").with_details(
                Details::new()
                    .with(Detail::FieldViolations(vec![FieldViolation::new("a", "negative")]))
                    .with(Detail::FieldViolations(vec![FieldViolation::new("b", "too large")]))
                    .with(Detail::Help(vec![Link::new("the rules", "https://example.com/rules")]))
                    .with(Detail::RetryAfter(Duration::from_secs(30))),
            )),
            -3 => Err(CallError::new(ErrorKind::NotFound, "missing")
                .with_details(Details::new().with(Detail::Json(serde_json::json!({ "k": [1, true, null], "s": "text" }))))),
            -4 => Err(CallError::new(ErrorKind::NotFound, "missing")),
            -5 => Err(CallError::new(ErrorKind::Conflict, "exists").with_grpc_code(i32::try_from(b).unwrap_or(0))),
            -6 => Err(CallError::new(ErrorKind::Conflict, "twice").with_details(
                Details::new().with(info("FIRST", "d1", &[])).with(info("SECOND", "d2", &[])),
            )),
            _ => Ok(Sum { sum: a + b }),
        }
    }
}

fn info(reason: &str, domain: &str, metadata: &[(&str, &str)]) -> Detail {
    let metadata: BTreeMap<String, String> = metadata.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    Detail::ErrorInfo { reason: reason.to_owned(), domain: domain.to_owned(), metadata }
}

/// The root of an app serving [`Service`], with its [`Hold`] bound.
pub struct Root(pub Hold);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Service>();
    }
}
