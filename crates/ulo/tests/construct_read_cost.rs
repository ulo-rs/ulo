//! Timing loop for the declared-read check on a construction (run by hand:
//! `cargo test --release -p ulo --test construct_read_cost -- --ignored --nocapture`).
//!
//! A transient hand-written `Construct` that reads three declared keys is built over and over
//! through one execution; the line printed is the median and spread of nanoseconds per build,
//! which includes the check on each of the three reads.

#[macro_use]
mod common;

use std::time::Instant;

use common::*;
use ulo::scope::Transient;
use ulo::{Construct, ConstructError, Dep, Dependencies, ExecOptions, Many, Resolver};

struct A;
struct One;
struct Two;

trait Plugin: Send + Sync {}
struct P;
impl Plugin for P {}

struct Reader;

impl Construct for Reader {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<One>>("one");
        d.field::<Dep<Two>>("two");
        d.field::<Many<dyn Plugin>>("plugins");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<One>().await?;
        r.dep::<Two>().await?;
        r.many::<dyn Plugin>().await?;
        Ok(Reader)
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "timing loop; run by hand in release mode"]
async fn nanoseconds_per_construction_with_three_declared_reads() {
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { One });
        m.singleton(|| async { Two });
        m.contribute::<dyn Plugin>().value(std::sync::Arc::new(P));
        m.provide::<Reader>();
    }))
    .await;

    const N: u32 = 200_000;
    let mut runs = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        app.execute(ExecOptions::new(), async |exec| {
            for _ in 0..N {
                exec.get::<Reader>().await.unwrap();
            }
        })
        .await
        .unwrap();
        runs.push(started.elapsed().as_nanos() as f64 / f64::from(N));
    }
    runs.sort_by(|a, b| a.total_cmp(b));
    println!("ns per construction: median {:.1}, min {:.1}, max {:.1}", runs[4], runs[0], runs[8]);
    close(app).await;
}
