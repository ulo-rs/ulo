use std::time::Duration;

use ex_12_standalone::{ReportJob, Tenant, builder, report};
use tokio::time::Instant;
use ulo::{Closed, ExecOptions, LookupError, Signal};
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn a_job_runs_in_its_own_execution_and_the_app_closes_with_no_servers() {
    let app = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();

    assert_eq!(report(&app, "acme").await.unwrap().unwrap(), "acme: 4 rows");
    assert_eq!(report(&app, "globex").await.unwrap().unwrap(), "globex: 6 rows", "each execution has its own Tenant");

    let handle = app.handle();
    let shutdown = app.close(Signal::new("done")).await.unwrap();
    assert_eq!(shutdown.signal.name(), "done");
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));

    // After the close the same handle refuses new executions.
    let late = handle.execute(ExecOptions::new(), async |_| ()).await;
    assert!(matches!(late, Err(Closed { .. })));
}

/// The app's clock is tokio's, so a paused test clock moves it: the deadline passes when the
/// runtime has nothing else to do, and no real time is spent waiting for it.
#[tokio::test(start_paused = true)]
async fn a_deadline_cancels_the_execution_on_the_apps_clock() {
    let deadline = Duration::from_secs(30);
    let app = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();

    let started = Instant::now();
    let options = ExecOptions::new().deadline((started + deadline).into_std());
    let ran = app
        .execute(options, async |exec| {
            exec.extensions().insert(Tenant("acme"));
            let job = exec.get::<ReportJob>().await?;
            exec.cancelled().await;
            Ok::<_, LookupError>(job.run())
        })
        .await;
    assert_eq!(ran.unwrap().unwrap(), "acme: 4 rows", "the job outlives the cancellation of its execution");
    assert_eq!(started.elapsed(), deadline);
    app.close(Signal::new("done")).await.unwrap();
}
