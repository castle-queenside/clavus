//! Clavus error types.
//!
//! Re-exports all error types for ergonomic use: `use clavus_core::ClavusResult`.

pub use crate::types::{ClavusError, SampleError};
pub type ClavusResult<T> = Result<T, ClavusError>;
