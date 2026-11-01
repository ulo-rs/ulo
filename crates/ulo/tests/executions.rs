//! Executions (section 6.3): what one carries, what ends it, and when the app refuses to open
//! another.
//!
//! Capabilities pinned: 23 (an execution carries a typed extension bag, a cancellation signal and
//! an optional deadline, and its `ExecutionRef` is injectable into execution-scoped services),
//! 12 (the per-execution-data site at run time: a missing extension or input), 13 (`Option` of
//! either is `None`), and the runtime rows of section 3: input not seeded or extension not
//! written, and `execute` refused from Draining on.
//!
//! Not pinned here: `open_terminal` during the drain. Its `DrainToken` is built only for a
//! `Server`'s `drain`, so the allowed half belongs to a test with a server fixture (batch B4).

#[macro_use]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use ulo::{
    App, CancelReason, Closed, Dep, ExecOptions, Execution, ExecutionRef, Ext, LookupError, LookupKind, Phase, Signal, Timer,
};

struct A;

#[derive(Debug, PartialEq)]
struct User(u8);

struct Addr;

struct Who {
    user: Ext<User>,
    exec: ExecutionRef,
}

fn who_module() -> Mod<A> {
    Mod::new(|m| {
        m.execution(|user: Ext<User>, exec: ExecutionRef| async move { Who { user, exec } });
    })
}

#[tokio::test]
async fn an_execution_scoped_service_reads_the_bag_and_the_execution_it_was_built_in() {
    let app = connected(who_module()).await;
    app.execute(ExecOptions::new(), async |exec| {
        exec.extensions().insert(User(7));
        let who = exec.get::<Who>().await.unwrap();
        assert_eq!(*who.user, User(7));
        assert!(Arc::ptr_eq(&exec.extensions().get::<User>().unwrap(), &exec.extensions().get::<User>().unwrap()));

        // The injected `ExecutionRef` is this execution: a write through it is the bag the holder reads.
        who.exec.extensions().insert(Addr);
        assert!(exec.extensions().contains::<Addr>());
        who.exec.cancel();
        assert!(exec.is_cancelled(), "and its signal is this execution's");
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn an_extension_nobody_wrote_is_not_found_and_an_optional_read_is_none() {
    struct Maybe(Option<Ext<User>>);
    struct Needs(#[allow(dead_code)] Ext<User>);
    let app = connected(Mod::<A>::new(|m| {
        m.execution(|user: Option<Ext<User>>| async move { Maybe(user) });
        m.execution(|user: Ext<User>| async move { Needs(user) });
    }))
    .await;
    app.execute(ExecOptions::new(), async |exec| {
        assert!(exec.get::<Maybe>().await.unwrap().0.is_none());
        match exec.get::<Needs>().await {
            Err(LookupError::NotFound { kind: LookupKind::Extension, .. }) => {}
            other => panic!("expected NotFound(Extension), found {:?}", other.map(|_| ())),
        }
        match exec.resolver().ext::<User>() {
            Err(LookupError::NotFound { kind: LookupKind::Extension, .. }) => {}
            other => panic!("expected NotFound(Extension), found {:?}", other.map(|_| ())),
        }
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn an_input_nobody_seeded_is_not_found_and_a_seeded_one_is_read() {
    struct Reads(Option<Dep<Addr>>);
    let app = connected(Mod::<A>::new(|m| {
        m.input::<Addr>().seeded_by::<TA>();
        m.execution(|addr: Option<Dep<Addr>>| async move { Reads(addr) });
    }))
    .await;
    assert!(
        matches!(app.get::<Addr>().await, Err(LookupError::ExecutionRequired { .. })),
        "no execution, no input"
    );
    app.execute(ExecOptions::new(), async |exec| {
        match exec.get::<Addr>().await {
            Err(LookupError::NotFound { kind: LookupKind::Input, .. }) => {}
            other => panic!("expected NotFound(Input), found {:?}", other.map(|_| ())),
        }
        assert!(exec.get::<Reads>().await.unwrap().0.is_none(), "an optional input is `None` unseeded");
    })
    .await
    .unwrap();
    app.execute(ExecOptions::new(), async |exec| {
        exec.seed(Addr);
        exec.get::<Addr>().await.expect("a seeded input");
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn cancel_fires_the_signal_once_and_the_first_reason_stays() {
    let app = connected(Empty).await;
    let handle = app
        .execute(ExecOptions::new(), async |exec| {
            let handle = exec.handle();
            assert!(!exec.is_cancelled() && exec.cancel_reason().is_none());
            exec.cancel();
            tokio::time::timeout(Duration::from_secs(5), exec.cancelled()).await.expect("cancelled() resolves once it fired");
            exec.cancel_with(CancelReason::Drain);
            assert_eq!(exec.cancel_reason(), Some(CancelReason::Explicit), "the first reason stays");
            handle
        })
        .await
        .unwrap();
    assert!(handle.is_cancelled(), "a handle sees what the holder did");
    assert_eq!(handle.cancel_reason(), Some(CancelReason::Explicit));
    close(app).await;
}

#[tokio::test]
async fn completing_an_execution_fires_no_cancellation() {
    let app = connected(Empty).await;
    let handle = app.execute(ExecOptions::new(), async |exec| exec.handle()).await.unwrap();
    assert!(!handle.is_cancelled());
    assert_eq!(handle.cancel_reason(), None);
    close(app).await;
}

#[tokio::test]
async fn the_deadline_is_carried_and_fires_cancellation_on_the_apps_timer() {
    let frozen = Frozen::new();
    let now = frozen.now();
    let soon = now + Duration::from_secs(1);
    let app = connected_with(App::builder(Empty).timer(frozen)).await;

    app.execute(ExecOptions::new().deadline(soon), async |exec| {
        assert_eq!(exec.deadline(), Some(soon));
        assert_eq!(exec.handle().deadline(), Some(soon));
        // The frozen timer ends the deadline's sleep at once.
        tokio::time::timeout(Duration::from_secs(5), exec.cancelled()).await.expect("the deadline fires cancellation");
        assert_eq!(exec.cancel_reason(), Some(CancelReason::Deadline));
    })
    .await
    .unwrap();

    app.execute(ExecOptions::new().deadline(now - Duration::from_secs(1)), async |exec| {
        assert!(exec.is_cancelled(), "a deadline already past is cancelled before the closure first runs");
        assert_eq!(exec.cancel_reason(), Some(CancelReason::Deadline));
    })
    .await
    .unwrap();

    app.execute(ExecOptions::new(), async |exec| {
        assert_eq!(exec.deadline(), None);
        assert!(!exec.is_cancelled());
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn without_a_timer_a_deadline_is_carried_and_never_fires() {
    let app = connected(Empty).await;
    let at = std::time::Instant::now();
    app.execute(ExecOptions::new().deadline(at), async |exec| {
        tokio::task::yield_now().await;
        assert_eq!(exec.deadline(), Some(at));
        assert!(!exec.is_cancelled(), "there is no clock to read it against");
    })
    .await
    .unwrap();
    close(app).await;
}

#[tokio::test]
async fn an_execution_opens_through_a_module_handle_with_that_modules_visibility() {
    let app = connected(who_module()).await;
    let module = app.module::<Mod<A>>().unwrap();
    let exec = Execution::open(&module, ExecOptions::new()).unwrap();
    exec.extensions().insert(User(3));
    assert_eq!(*exec.get::<Who>().await.unwrap().user, User(3));
    drop(exec);
    close(app).await;
}

#[tokio::test]
async fn draining_refuses_new_executions() {
    let app = connected_on_tokio(Mod::<A>::bare()).await;
    let handle = app.handle();
    let root = handle.root();
    let mut closing = None;
    handle
        .execute(ExecOptions::new(), async |exec| {
            closing = Some(tokio::spawn(handle.close(Signal::new("test"))));
            tokio::time::timeout(Duration::from_secs(5), exec.draining()).await.expect("the notice arrives");
            assert!(exec.is_draining());
            assert_eq!(handle.phase(), Phase::Draining);
            assert!(matches!(handle.execute(ExecOptions::new(), async |_| ()).await, Err(Closed { .. })));
            assert!(matches!(Execution::open(&root, ExecOptions::new()), Err(Closed { .. })));
        })
        .await
        .unwrap();
    closing.expect("close was started").await.expect("the close task ran").expect("the app closed");
    assert_eq!(handle.phase(), Phase::Closed);
    assert!(matches!(handle.execute(ExecOptions::new(), async |_| ()).await, Err(Closed { .. })), "and stays refused once closed");
    drop(app);
}
