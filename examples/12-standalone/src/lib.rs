use ulo::{App, AppBuilder, Closed, Connected, Dep, ExecOptions, Ext, LookupError, injectable, module};

/// Per-execution data the caller writes before asking for the job.
pub struct Tenant(pub &'static str);

#[injectable]
pub struct Ledger;

impl Ledger {
    fn rows(&self, tenant: &str) -> u32 {
        tenant.len() as u32
    }
}

/// Execution-scoped: one `ReportJob` per `execute`, reading what the caller put in the
/// execution's extensions and the singleton `Ledger`.
#[injectable(execution)]
pub struct ReportJob {
    tenant: Ext<Tenant>,
    ledger: Dep<Ledger>,
}

impl ReportJob {
    pub fn run(&self) -> String {
        format!("{}: {} rows", self.tenant.0, self.ledger.rows(self.tenant.0))
    }
}

#[module(providers = [Ledger, ReportJob])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}

/// Opens one execution on the app, hands it `tenant`, and runs the report in it.
pub async fn report(app: &App<Connected>, tenant: &'static str) -> Result<Result<String, LookupError>, Closed> {
    app.execute(ExecOptions::new(), async |exec| {
        exec.extensions().insert(Tenant(tenant));
        Ok(exec.get::<ReportJob>().await?.run())
    })
    .await
}
