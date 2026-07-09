//! Shared tool error. Mirrors `ToolError` in `flows/tools/files.py`, used by
//! every tool. Dispatch turns these into `"Error: {msg}"` strings for the model.

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(msg: impl Into<String>) -> Self {
        ToolError(msg.into())
    }
}

pub type ToolResult<T> = Result<T, ToolError>;
