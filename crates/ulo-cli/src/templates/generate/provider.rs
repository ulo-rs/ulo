use ulo::injectable;

/// Built once by the container and shared. Give it fields typed `Dep<..>` to inject what it needs,
/// bind it with `m.provide::<@@pascal@@>()` and read it as `Dep<@@pascal@@>`.
#[injectable]
pub struct @@pascal@@;

#[cfg(test)]
mod tests {
    use ulo::{Module, ModuleDef, ModuleIdentity, TestApp};

    struct Root;

    impl Module for Root {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, m: &mut ModuleDef<'_>) {
            m.provide::<super::@@pascal@@>();
        }
    }

    #[tokio::test]
    async fn is_built_by_the_container() {
        let app = TestApp::of(Root).connect().await.expect("the application connects");

        let _provider = app.get::<super::@@pascal@@>().await.expect("the provider resolves");
    }
}
