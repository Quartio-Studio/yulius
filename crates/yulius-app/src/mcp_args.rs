//! The MCP tools' descriptors and the arguments they read.

use serde_json::{Value, json};
use yulius_contracts::mcp::ToolDescriptor;

use crate::error::{AppError, AppResult};

/// A tool's input schema: an object of `properties`, `required` ones named, nothing else taken.
fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// A tool's descriptor.
pub(crate) fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema: schema(properties, required),
    }
}

/// The text argument `key`, trimmed.
///
/// # Errors
/// `Invalid` when it is missing, not text or blank.
pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| missing(key))
}

/// The text argument `key` as given, blank included.
///
/// # Errors
/// `Invalid` when it is missing or not text.
pub(crate) fn text_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| missing(key))
}

fn missing(key: &str) -> AppError {
    AppError::Invalid(format!("missing string argument `{key}`"))
}

/// The optional text argument `key`, trimmed; blank is none.
///
/// # Errors
/// `Invalid` when it is given but not text.
pub(crate) fn opt_str(args: &Value, key: &str) -> AppResult<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Some(_) => Err(AppError::Invalid(format!("`{key}` must be a string"))),
    }
}

/// The optional flag `key`, false when absent.
///
/// # Errors
/// `Invalid` when it is given but not a boolean.
pub(crate) fn flag_arg(args: &Value, key: &str) -> AppResult<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(AppError::Invalid(format!("`{key}` must be true or false"))),
    }
}
