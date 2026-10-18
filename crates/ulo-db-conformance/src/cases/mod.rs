//! The scenarios of integrations DESIGN §4.9, one function per scenario, each generic over the
//! [`Host`](crate::Host) and each naming the planted break it is shown failing against.

pub mod close;
pub mod connect;
pub mod health;
pub mod keyed;
pub mod lazy;
pub mod migrate;
pub mod runtime;
pub mod stall;
pub mod wire;
