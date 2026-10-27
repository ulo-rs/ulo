//! A server's link on an app that handed it no `Timer` refuses to listen with the `NoTimer` every
//! link answers, naming TCP, before any I/O.

use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{Link, NoTimer, Pattern};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[test]
fn a_server_on_an_app_with_no_timer_refuses_to_listen() {
    OnSmol::block_on(refused());
}

async fn refused() {
    let app = App::builder(Root).wire().expect("the app wires").connect().await.expect("the app connects");
    assert!(app.handle().timer().is_none(), "the app has a timer");
    let mut link = Tcp::new("127.0.0.1:0").with_runtime(OnSmol::runtime());
    link.prepare(&app.handle()).await.expect("the link prepares");
    let Err(refused) = link.listen(&[Pattern::from("no_timer.add")]).await else {
        panic!("a link listened on an app with no timer");
    };
    let no_timer = refused.downcast_ref::<NoTimer>().unwrap_or_else(|| panic!("the refusal is not a `NoTimer`: {refused}"));
    assert_eq!(no_timer.link, "TCP");
}
