use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, Dep, injectable, module, routes};
use ulo_rpc::{Classify, Payload};

mod roles;
pub use roles::{RequiresRole, RoleGuard};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Divide {
    pub dividend: i64,
    pub divisor: i64,
}

#[derive(Debug, Classify)]
pub enum CalcError {
    #[classify(unprocessable)]
    DivideByZero,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the divisor is zero")
    }
}

impl std::error::Error for CalcError {}

/// What the event handler heard, in arrival order.
pub struct Journal(Mutex<Vec<String>>);

#[injectable]
impl Journal {
    fn new() -> Self {
        Journal(Mutex::default())
    }

    pub fn clear(&self) -> usize {
        std::mem::take(&mut *self.0.lock().expect("the journal is not poisoned")).len()
    }

    pub fn entries(&self) -> Vec<String> {
        self.0.lock().expect("the journal is not poisoned").clone()
    }
}

#[injectable]
pub struct Calculator {
    journal: Dep<Journal>,
}

/// A `message` answers its caller; an `event` answers nobody. `RoleGuard` guards every handler,
/// and only those declaring `#[meta(RequiresRole(..))]` ask for anything.
#[routes]
#[guards(RoleGuard)]
impl Calculator {
    #[ulo_rpc::message("calc.divide")]
    async fn divide(&self, req: Payload<Divide>) -> Result<i64, CalcError> {
        req.0.dividend.checked_div(req.0.divisor).ok_or(CalcError::DivideByZero)
    }

    #[ulo_rpc::message("calc.clear")]
    #[meta(RequiresRole("admin"))]
    async fn clear(&self) -> usize {
        self.journal.clear()
    }

    #[ulo_rpc::event("calc.note")]
    async fn note(&self, text: Payload<String>) -> Result<(), CalcError> {
        self.journal.0.lock().expect("the journal is not poisoned").push(text.0);
        Ok(())
    }
}

#[module(providers = [Journal, RoleGuard], controllers = [Calculator])]
pub struct AppModule;

/// The application, before a runtime is chosen: `main` and the tests each name theirs.
pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
