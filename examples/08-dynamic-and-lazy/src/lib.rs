use std::sync::Mutex;

use ulo::{App, AppBuilder, Dep, DynamicModule, injectable, module, routes};

/// Records what the container builds, so a test can see when a lazy module's providers are built.
#[derive(Default)]
pub struct BuildLog(Mutex<Vec<&'static str>>);

impl BuildLog {
    pub fn push(&self, built: &'static str) {
        self.0.lock().unwrap().push(built);
    }

    pub fn entries(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

#[module(global, providers = [BuildLog::default()], exports = [BuildLog])]
pub struct LogModule;

pub struct Feature(pub String);

/// The owner type of every module `feature_module` builds; with the configuration it forms the
/// module's identity.
pub struct FeatureOwner;

/// A module built at runtime from a value, with no struct of its own.
pub fn feature_module(name: String) -> DynamicModule {
    DynamicModule::new::<FeatureOwner, _>(name.clone(), move |m| {
        m.value(Feature(name));
        m.export::<Feature>();
    })
    .label("FeatureModule")
}

pub struct Reports;

#[injectable]
impl Reports {
    fn new(log: Dep<BuildLog>) -> Self {
        log.push("Reports");
        Reports
    }

    pub fn title(&self) -> &'static str {
        "quarterly"
    }
}

/// Not imported by the application: `AppHandle::load` brings it in after startup.
#[module(providers = [Reports], exports = [Reports])]
pub struct ReportsModule;

#[injectable]
pub struct FeatureController {
    feature: Dep<Feature>,
}

#[routes]
impl FeatureController {
    #[ulo_http::get("/feature")]
    fn feature(&self) -> String {
        self.feature.0.clone()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module(imports = [LogModule, feature_module(self.feature.clone())], controllers = [FeatureController])]
pub struct AppModule {
    pub feature: String,
}

pub fn builder(feature: &str) -> AppBuilder {
    App::builder(AppModule { feature: feature.to_owned() })
}
