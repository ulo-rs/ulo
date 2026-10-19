//! The `close` claim raced across OS threads: the `App` value's drop against `close` calls made on
//! handle clones, each awaited, dropped unpolled, or polled once and dropped. Whatever the order,
//! the server drops once, its `close` runs at most once, the store is freed with the last handle,
//! and a `close` answering `Ok` closed the server while one answering `Dropped` saw it unclosed.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Waker};

use ulo::app::Bound as Serving;
use ulo::{App, BoxError, DrainToken, Module, ModuleDef, ModuleIdentity, Mounted, Server, ShutdownFailure, Signal, Transport};
use ulo_runtime_tokio::Tokio;

const ITERATIONS: u64 = 2000;

#[derive(Default)]
struct Counts {
    closes: AtomicUsize,
    drops: AtomicUsize,
    hooks: AtomicUsize,
    store: AtomicUsize,
}

struct Bare;

impl Transport for Bare {
    const KEY: &'static str = "bare";
    type Cx = ();
    type Reply = ();
}

/// Keeps the `AppHandle` its `prepare` is handed, as a shipped server does.
struct Counted(Arc<Counts>, Option<ulo::AppHandle>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl Server for Counted {
    type Transport = Bare;
    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.1 = Some(mounted.app().clone());
        Ok(())
    }
    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }
    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }
    async fn drain(&self, _token: DrainToken) {}
    async fn close(&self) -> Result<(), BoxError> {
        self.0.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Store(Arc<Counts>);

impl Drop for Store {
    fn drop(&mut self) {
        self.0.store.fetch_add(1, Ordering::SeqCst);
    }
}

struct Root(Arc<Counts>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }
    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Store(Arc::clone(&self.0)));
        let counts = Arc::clone(&self.0);
        m.before_shutdown(move |_signal: Signal| {
            counts.hooks.fetch_add(1, Ordering::SeqCst);
            async {}
        });
    }
}

/// What one `close` answered: `Some(true)` for `Ok`, `Some(false)` for `Dropped`, `None` unended.
type Answer = Option<bool>;

fn answer(outcome: Result<ulo::Shutdown, ulo::ShutdownError>) -> bool {
    match outcome {
        Ok(_) => true,
        Err(error) if matches!(&*error.failures, [ShutdownFailure::Dropped]) => false,
        Err(error) => panic!("a close answered {error}"),
    }
}

/// One `close` on its own thread, called and then settled after the two spins: `fate` 0 awaits
/// it, 1 drops it unpolled, 2 polls it once and drops it.
fn closer(handle: ulo::AppHandle, fate: u64, spin: (u64, u64), barrier: Arc<Barrier>, rt: tokio::runtime::Handle) -> Answer {
    let _entered = rt.enter();
    barrier.wait();
    (0..spin.0).for_each(|_| std::hint::spin_loop());
    let closing = handle.close(Signal::new("stress"));
    drop(handle);
    (0..spin.1).for_each(|_| std::hint::spin_loop());
    match fate {
        0 => Some(answer(futures_executor::block_on(closing))),
        1 => None,
        _ => match pin!(closing).as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(outcome) => Some(answer(outcome)),
            Poll::Pending => None,
        },
    }
}

#[test]
fn every_order_of_claims_and_the_drop_closes_or_releases_the_server_once() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let (mut oks, mut dropped) = (0, 0);
    for iteration in 0..ITERATIONS {
        let counts = Arc::new(Counts::default());
        let app: App<Serving> = rt.block_on(async {
            let builder = App::builder(Root(Arc::clone(&counts))).runtime(Tokio::current());
            builder.wire().unwrap().connect().await.unwrap().bind(Counted(Arc::clone(&counts), None)).listen().await.unwrap()
        });
        let closers = 1 + next(4) as usize;
        let barrier = Arc::new(Barrier::new(closers + 1));
        let threads: Vec<_> = (0..closers)
            .map(|_| {
                let (handle, fate, spin, barrier, rt) = (app.handle(), next(3), (next(2000), next(2000)), Arc::clone(&barrier), rt.handle().clone());
                std::thread::spawn(move || closer(handle, fate, spin, barrier, rt))
            })
            .collect();
        let spin = next(2000);
        barrier.wait();
        (0..spin).for_each(|_| std::hint::spin_loop());
        drop(app);
        let answers: Vec<Answer> = threads.into_iter().map(|t| t.join().expect("a closer panicked")).collect();
        let state = |c: &AtomicUsize| c.load(Ordering::SeqCst);
        let (closes, drops, hooks, store) = (state(&counts.closes), state(&counts.drops), state(&counts.hooks), state(&counts.store));
        let at = format!("iteration {iteration}: answers {answers:?}, closes {closes}, drops {drops}, hooks {hooks}, store {store}");
        assert_eq!(drops, 1, "the server did not drop once: {at}");
        assert!(closes <= 1 && hooks <= 1, "a step ran twice: {at}");
        assert_eq!(store, 1, "the store was not freed: {at}");
        for answered in answers.into_iter().flatten() {
            assert_eq!(closes, usize::from(answered), "a close answered `{}` against the server's closes: {at}", if answered { "Ok" } else { "Dropped" });
            if answered { oks += 1 } else { dropped += 1 }
        }
    }
    println!("{ITERATIONS} iterations: {oks} closes answered Ok, {dropped} answered Dropped");
    assert!(oks > 0 && dropped > 0, "the races reached only one answer");
}
