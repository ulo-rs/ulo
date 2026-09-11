// Tests: none, and none anywhere else either — this is the one database
// integration with no startup check and no health indicator, so it shares
// neither suite with the other five. Tracked in `coverage_ledger.rs`.

mod client;
mod module;

pub use module::PrismaModule;
