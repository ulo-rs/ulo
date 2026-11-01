use std::time::Duration;

use ulo::{App, AppBuilder, Dep, Timer, injectable, module, routes};

pub trait Mailer: Send + Sync {
    fn send(&self, to: &str) -> String;
}

#[injectable]
pub struct SmtpMailer;

impl Mailer for SmtpMailer {
    fn send(&self, to: &str) -> String {
        format!("smtp to {to}")
    }
}

#[module(providers = [SmtpMailer as dyn Mailer], exports = [dyn Mailer])]
pub struct MailModule;

/// Two modules each bind a `Ledger` privately.
pub struct Ledger(pub &'static str);

#[module(providers = [Ledger("billing")], exports = [Ledger])]
pub struct BillingModule;

#[module(providers = [Ledger("refunds")])]
pub struct RefundsModule;

pub struct Stamp(pub String);

/// Time since startup, read from the app's `Timer` rather than the system clock.
pub struct Uptime {
    started: std::time::Instant,
    timer: Dep<dyn Timer>,
}

#[injectable]
impl Uptime {
    fn new(timer: Dep<dyn Timer>) -> Self {
        Uptime { started: timer.now(), timer }
    }

    pub fn elapsed(&self) -> Duration {
        self.timer.now() - self.started
    }
}

#[injectable]
pub struct SignupController {
    mailer: Dep<dyn Mailer>,
    stamp: Dep<Stamp>,
    ledger: Dep<Ledger>,
}

#[routes]
impl SignupController {
    #[ulo_http::get("/signup")]
    fn signup(&self) -> String {
        format!("{} at {} in {}", self.mailer.send("ada"), self.stamp.0, self.ledger.0)
    }
}

#[module(
    imports = [MailModule, BillingModule, RefundsModule],
    providers = [with = || Stamp("production".to_owned()), Uptime],
    controllers = [SignupController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
