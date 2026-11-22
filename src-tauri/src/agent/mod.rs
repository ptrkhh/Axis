//! Tauri-free command dispatch for the remote agent: `{command, args}` JSON in, JSON out.

mod dispatch;

use crate::error::AxisError;
use crate::state::AppState;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub use dispatch::{command_names, dispatch};

/// Reply of one command: the serialized output, or the error as Tauri sends it.
pub type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, Value>> + Send + 'a>>;

/// The agent entry point that `axis_command!` generates as `<command>::call`.
pub type CommandFn = for<'a> fn(&'a AppState, Value) -> CommandFuture<'a>;

/// Serialize an error exactly as Tauri serializes command errors for the frontend.
pub(crate) fn error_value(err: &AxisError) -> Value {
    serde_json::to_value(err).unwrap_or_else(|e| {
        log::error!("Failed to serialize error {err}: {e}");
        Value::String(err.to_string())
    })
}

pub(crate) fn parse_args<T: DeserializeOwned>(command: &str, args: Value) -> Result<T, Value> {
    serde_json::from_value(args).map_err(|e| {
        log::warn!("Invalid arguments for agent command {command}: {e}");
        error_value(&AxisError::SerializationError(e.to_string()))
    })
}

pub(crate) fn into_reply<T: Serialize>(result: crate::error::Result<T>) -> Result<Value, Value> {
    let output = result.map_err(|e| error_value(&e))?;
    serde_json::to_value(output).map_err(|e| {
        log::error!("Failed to serialize agent command output: {e}");
        error_value(&AxisError::from(e))
    })
}
