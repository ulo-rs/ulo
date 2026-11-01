//! Keys an `into K: [..]` list accepts: the metadata check `#[module]` writes before the list
//! must pass a trait-object key and a role key. The refusals for a metadata key are the
//! `into_module_metadata_*` cases in `tests/ui`.

use ulo::{App, Connected, ExecOptions, injectable, module};

trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

#[injectable]
struct Alpha;

impl Plugin for Alpha {
    fn name(&self) -> &'static str {
        "alpha"
    }
}

#[module(providers = [into dyn Plugin: [Alpha, value = Alpha, with = || Alpha]])]
struct Plugins;

async fn connected(root: impl ulo::Module) -> App<Connected> {
    App::builder(root).wire().expect("wires").connect().await.expect("connects")
}

#[tokio::test]
async fn a_trait_object_key_still_takes_contributions() {
    let app = connected(Plugins).await;
    let all = app
        .execute(ExecOptions::new(), async |exec| exec.resolver().many::<dyn Plugin>().await)
        .await
        .expect("a connected app opens executions")
        .expect("the collection resolves");
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|plugin| plugin.name() == "alpha"));
}
