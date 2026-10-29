use ex_03_traits_and_qualifiers::{Db, DbModule, PgUserRepo, UserRepo, builder};
use ex_support::{get, start};
use ulo::{App, Dep, StartupError, WiringError, injectable, module, routes};

#[tokio::test]
async fn each_handler_reads_the_instance_its_type_or_qualifier_names() {
    let server = start(builder()).await;

    assert_eq!(get(server.addr, "/repo").await.body, "pg");
    assert_eq!(get(server.addr, "/write").await.body, "primary");
    assert_eq!(get(server.addr, "/read").await.body, "replica");

    let repo: Dep<dyn UserRepo> = server.handle.get().await.unwrap();
    assert_eq!(repo.name(), "pg");
    // `as dyn UserRepo` leaves the concrete key bound beside the trait key.
    let concrete: Dep<PgUserRepo> = server.handle.get().await.unwrap();
    assert!(std::ptr::addr_eq(std::sync::Arc::as_ptr(concrete.arc()), std::sync::Arc::as_ptr(repo.arc())));

    server.stop().await.unwrap();
}

#[injectable]
struct Unqualified {
    _db: Dep<Db>,
}

#[routes]
impl Unqualified {
    #[ulo_http::get("/")]
    fn read(&self) -> &'static str {
        "never"
    }
}

#[module(imports = [DbModule], controllers = [Unqualified])]
struct UnqualifiedModule;

#[test]
fn reading_the_unqualified_key_is_a_missing_dependency() {
    let Err(StartupError::Wiring(errors)) = App::builder(UnqualifiedModule).wire() else {
        panic!("wiring should fail");
    };
    let [WiringError::Missing { key, consumer, module, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one missing dependency, got {errors}");
    };
    assert_eq!(key.to_string(), "Db");
    assert_eq!(consumer, "Unqualified (field `_db`)");
    assert_eq!(module.to_string(), "UnqualifiedModule");
}
