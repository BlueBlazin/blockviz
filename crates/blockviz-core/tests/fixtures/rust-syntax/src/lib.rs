//! A tour of Rust syntax, editions 2015 through 2024, for the parser tests.
#![allow(dead_code, unused)]

pub mod modern;
pub mod macros;
pub mod legacy;
pub mod net;
#[path = "elsewhere.rs"]
pub mod moved;

pub use modern::{Shape, Stack};
pub use net::client::Client;

/// The library's result type.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const VERSION: &str = "1.0";
