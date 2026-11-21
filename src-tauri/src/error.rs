//! Error type that keeps the message shown to the user separate from the
//! detail written to the log. Never put passwords or private key material
//! into either field.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
#[error("{user_message}")]
pub struct AppError {
    /// Short, actionable text for the UI (Korean).
    pub user_message: String,
    /// Technical detail for logs only.
    pub detail: String,
}

impl AppError {
    pub fn new(user_message: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        let e = Self {
            user_message: user_message.into(),
            detail: detail.to_string(),
        };
        tracing::warn!(user_message = %e.user_message, detail = %e.detail, "app error");
        e
    }
}

/// Only the user message crosses the IPC boundary.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.user_message)
    }
}
