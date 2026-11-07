use ulo::{controller, handlers};

/// RPC patterns under `@@pattern@@`. Bind it with `m.controller::<@@pascal@@Controller>()`.
#[controller]
pub struct @@pascal@@Controller;

#[handlers]
impl @@pascal@@Controller {
    #[ulo_rpc::message("@@pattern@@.list")]
    async fn list(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::@@pascal@@Controller;

    #[tokio::test]
    async fn lists_nothing_yet() {
        assert!(@@pascal@@Controller.list().await.is_empty());
    }
}
