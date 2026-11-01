use ex_06_modules::{PasswordHasher, UsersModule, builder};
use ex_support::{get, start};
use ulo::{App, Dep, StartupError, WiringError, injectable, module, routes};

#[tokio::test]
async fn billing_reads_what_users_exports() {
    let server = start(builder()).await;
    assert_eq!(get(server.addr, "/").await.body, "invoice for ada registered with hashed:secret");
    server.stop().await.unwrap();
}

#[injectable]
struct Snooper {
    _hasher: Dep<PasswordHasher>,
}

#[routes]
impl Snooper {
    #[ulo_http::get("/")]
    fn snoop(&self) -> &'static str {
        "never"
    }
}

#[module(imports = [UsersModule], controllers = [Snooper])]
struct SnoopingModule;

#[test]
fn reading_a_private_binding_fails_wiring_with_the_key_and_the_reader() {
    let Err(StartupError::Wiring(errors)) = App::builder(SnoopingModule).wire() else {
        panic!("wiring should fail");
    };
    let report = errors.to_string();
    assert!(report.contains("missing dependency `PasswordHasher`"), "{report}");
    assert!(report.contains("needed by Snooper (field `_hasher`) in SnoopingModule"), "{report}");
    let [WiringError::Missing { key, consumer, module, near }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one missing dependency, got {errors}");
    };
    assert_eq!(key.to_string(), "PasswordHasher");
    assert_eq!(consumer, "Snooper (field `_hasher`)");
    assert_eq!(module.to_string(), "SnoopingModule");
    assert!(near.is_none());
}
