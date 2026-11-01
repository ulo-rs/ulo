use ulo::{App, AppBuilder, Dep, injectable, module, routes};

/// Private to `UsersModule`: bound there and not exported.
#[injectable]
pub struct PasswordHasher;

impl PasswordHasher {
    pub fn hash(&self, password: &str) -> String {
        format!("hashed:{password}")
    }
}

/// Exported: other modules may read it.
#[injectable]
pub struct UserService {
    hasher: Dep<PasswordHasher>,
}

impl UserService {
    pub fn register(&self, name: &str) -> String {
        format!("{name} registered with {}", self.hasher.hash("secret"))
    }
}

#[module(providers = [PasswordHasher, UserService], exports = [UserService])]
pub struct UsersModule;

#[injectable]
pub struct InvoiceService {
    users: Dep<UserService>,
}

#[injectable]
pub struct InvoiceController {
    invoices: Dep<InvoiceService>,
}

impl InvoiceService {
    pub fn open_for(&self, name: &str) -> String {
        format!("invoice for {}", self.users.register(name))
    }
}

#[routes]
impl InvoiceController {
    #[ulo_http::get("/")]
    fn open(&self) -> String {
        self.invoices.open_for("ada")
    }
}

/// Sees what `UsersModule` exports and nothing else of it.
#[module(imports = [UsersModule], providers = [InvoiceService], controllers = [InvoiceController])]
pub struct BillingModule;

#[module(imports = [BillingModule])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
