//! A `value = expr` written on a `#[routes]` impl is built once for the whole impl and shared by
//! every handler on it (transports DESIGN X2); written on a method it is that handler's alone.
//!
//! Each guard instance takes an id from a counter when its `new` runs and records `(id, n)` when
//! it admits a call, `n` being how many calls that instance has admitted. One shared instance
//! therefore records `(id, 1)` then `(id, 2)` across two handlers; a build per handler records
//! `(id, 1)` and `(id', 1)`.

mod support;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{BoxError, Guard, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::probe::vault_client::VaultClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message};

use support::Running;

/// Defines a guard `$name` with a module's worth of statics of its own, so the two tests in this
/// file cannot see each other's instances.
macro_rules! tally {
    ($module:ident) => {
        mod $module {
            use super::*;

            static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
            static ADMITTED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

            pub struct Tally {
                id: usize,
                served: AtomicUsize,
            }

            impl Tally {
                pub fn new() -> Self {
                    Tally { id: NEXT_ID.fetch_add(1, Ordering::SeqCst), served: AtomicUsize::new(0) }
                }

                /// How many instances `new` has made.
                pub fn built() -> usize {
                    NEXT_ID.load(Ordering::SeqCst) - 1
                }

                /// `(instance id, calls admitted by it so far)` in admission order.
                pub fn admitted() -> Vec<(usize, usize)> {
                    ADMITTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
                }
            }

            impl Guard<Grpc> for Tally {
                async fn can_activate(&self, _cx: &GrpcCx) -> Result<bool, BoxError> {
                    let n = self.served.fetch_add(1, Ordering::SeqCst) + 1;
                    ADMITTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((self.id, n));
                    Ok(true)
                }
            }
        }
    };
}

tally!(on_the_impl);
tally!(on_the_methods);

#[injectable]
struct Shared;

#[routes]
#[guards(value = on_the_impl::Tally::new())]
impl Shared {
    #[ulo_grpc::method(probe::vault::Open)]
    fn open(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }

    #[ulo_grpc::method(probe::vault::Sealed)]
    fn sealed(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }
}

#[injectable]
struct PerMethod;

#[routes]
impl PerMethod {
    #[ulo_grpc::method(probe::vault::Open)]
    #[guards(value = on_the_methods::Tally::new())]
    fn open(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }

    #[ulo_grpc::method(probe::vault::Sealed)]
    #[guards(value = on_the_methods::Tally::new())]
    fn sealed(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }
}

struct Root<C: ulo::Controller + ulo::Construct>(std::marker::PhantomData<fn() -> C>);

impl<C: ulo::Controller + ulo::Construct + 'static> Module for Root<C> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<C>();
    }
}

fn root<C: ulo::Controller + ulo::Construct + 'static>() -> Root<C> {
    Root(std::marker::PhantomData)
}

async fn call_both(running: &Running) {
    let mut client = VaultClient::new(running.channel().await);
    let open = client.open(Ticks { count: 1, fail: false }).await.unwrap_or_else(|status| panic!("Open failed: {status:?}"));
    assert_eq!(open.into_inner(), Tick { n: 1 });
    let sealed = client.sealed(Ticks { count: 2, fail: false }).await.unwrap_or_else(|status| panic!("Sealed failed: {status:?}"));
    assert_eq!(sealed.into_inner(), Tick { n: 2 });
}

#[tokio::test]
async fn an_impl_level_value_is_one_instance_shared_by_every_handler_of_the_impl() {
    let app = Running::start(root::<Shared>(), ulo_grpc::Server::new("127.0.0.1:0")).await;
    call_both(&app).await;
    app.stop().await;

    assert_eq!(on_the_impl::Tally::built(), 1, "the value was built once for the impl");
    let admitted = on_the_impl::Tally::admitted();
    let id = admitted.first().map(|(id, _)| *id).expect("the guard admitted a call");
    assert_eq!(admitted, vec![(id, 1), (id, 2)], "both handlers admitted through one instance, which counted two");
}

#[tokio::test]
async fn a_method_level_value_is_that_handler_s_own_instance() {
    let app = Running::start(root::<PerMethod>(), ulo_grpc::Server::new("127.0.0.1:0")).await;
    call_both(&app).await;
    app.stop().await;

    assert_eq!(on_the_methods::Tally::built(), 2, "one value per method");
    let admitted = on_the_methods::Tally::admitted();
    assert_eq!(admitted.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![1, 1], "each instance admitted one call");
    assert_ne!(admitted[0].0, admitted[1].0, "the two handlers hold two instances");
}
