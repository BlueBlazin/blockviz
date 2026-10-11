//! Networking: a client and its wire format.
pub mod client;
mod wire;

pub(crate) use self::wire::encode;
