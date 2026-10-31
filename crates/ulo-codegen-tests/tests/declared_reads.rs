//! A generated `Construct` declares every injection point it reads, so the declared-read check
//! (DESIGN section 3.4) never refuses it: a field of each shape constructs.

use ulo::{App, Dep, ExecOptions, Ext, Many, injectable, module};

struct Primary;
struct Absent;
struct Db(&'static str);
struct Caller(&'static str);

trait Plugin: Send + Sync {}

#[injectable]
struct One;

impl Plugin for One {}

#[injectable]
struct Reader {
    plain: Dep<One>,
    absent: Option<Dep<Absent>>,
    plugins: Many<dyn Plugin>,
    primary: Dep<Db, Primary>,
}

#[injectable(execution)]
struct PerCall {
    caller: Ext<Caller>,
}

#[module(providers = [One, Db("primary") @ Primary, Reader, PerCall])]
struct Root;

#[tokio::test]
async fn generated_constructors_read_each_declared_shape() {
    let app = App::builder(Root).wire().expect("wires").connect().await.expect("connects");

    let reader = app.get::<Reader>().await.expect("the generated constructor reads only what it declared");
    assert!(reader.absent.is_none());
    assert_eq!(reader.primary.0, "primary");
    assert!(reader.plugins.is_empty());
    let _ = &reader.plain;

    let caller = app
        .execute(ExecOptions::new(), async |exec| {
            exec.extensions().insert(Caller("ann"));
            exec.get::<PerCall>().await.map(|call| call.caller.0)
        })
        .await
        .unwrap();
    assert_eq!(caller.unwrap(), "ann");
}
