//! `Redacted::original` (§10.2, §14 item 67): the unredacted error a constructor returned, and the
//! `source()` chain below it, which the `Redacted`'s own `source()` stops short of. A failure an
//! execution keeps for every reader (§3.8) is reached the same way from each of them.

#[macro_use]
mod common;

use std::error::Error;
use std::fmt;
use std::io;

use common::*;
use ulo::{ExecOptions, FailureReason, LookupError, Redacted};

struct A;
struct Svc;

/// A domain error whose text carries a credential the redaction strips, over an I/O cause.
#[derive(Debug)]
struct TenantMissing {
    cause: io::Error,
}

impl fmt::Display for TenantMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no tenant `acme` at postgres://admin:hunter2@db:5432")
    }
}

impl Error for TenantMissing {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

fn missing() -> TenantMissing {
    TenantMissing { cause: io::Error::new(io::ErrorKind::ConnectionRefused, "the tenant store refused") }
}

/// The `Redacted` a failed build inside a call holds.
fn errored(answer: Result<ulo::Dep<Svc>, LookupError>) -> Redacted {
    let Err(LookupError::Construct { reason, .. }) = answer else { panic!("the build did not fail") };
    let FailureReason::Errored(redacted) = *reason else { panic!("the build failed other than by its error: {reason}") };
    redacted
}

/// The original's chain, as the `source()` links below it.
fn causes(original: &(dyn Error + 'static)) -> Vec<String> {
    let mut texts = Vec::new();
    let mut next = original.source();
    while let Some(cause) = next {
        texts.push(cause.to_string());
        next = cause.source();
    }
    texts
}

#[tokio::test]
async fn original_hands_over_the_constructor_s_error_and_its_chain() {
    let app = connected(Mod::<A>::new(|m| {
        m.try_transient(|| async { Err::<Svc, _>(missing()) });
    }))
    .await;
    let redacted = app.execute(ExecOptions::new(), async |exec| errored(exec.get::<Svc>().await)).await.unwrap();

    assert!(!redacted.to_string().contains("hunter2"), "the redacted text keeps the credential: {redacted}");
    assert!(redacted.source().is_none(), "a `Redacted` hands its original to no chain reporter");
    let original = redacted.original();
    assert!(original.is::<TenantMissing>(), "the original is not the constructor's error: {original:?}");
    assert!(original.to_string().contains("hunter2"), "the original is the unredacted error");
    assert_eq!(causes(original), ["the tenant store refused"], "the original's own chain");
    close(app).await;
}

/// Planted break: `original` answering the shared wrapper rather than the error it holds.
#[tokio::test]
async fn original_hands_every_reader_of_a_kept_failure_the_one_shared_error() {
    let app = connected(Mod::<A>::new(|m| {
        m.try_execution(|| async { Err::<Svc, _>(missing()) });
    }))
    .await;
    let (first, second) = app
        .execute(ExecOptions::new(), async |exec| (errored(exec.get::<Svc>().await), errored(exec.get::<Svc>().await)))
        .await
        .unwrap();

    for redacted in [&first, &second] {
        let original = redacted.original();
        assert!(original.downcast_ref::<TenantMissing>().is_some(), "a shared original is not the constructor's error: {original:?}");
        assert_eq!(causes(original), ["the tenant store refused"], "the shared original's own chain");
    }
    let (a, b) = (first.original() as *const dyn Error as *const (), second.original() as *const dyn Error as *const ());
    assert_eq!(a, b, "the two readers of one kept failure hold one original");
    close(app).await;
}
