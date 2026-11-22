// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<sqlx::Error> for ServiceError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ServiceError::Internal(e.to_string())
    }
}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self {
        ServiceError::Internal(e.to_string())
    }
}

pub type SResult<T> = Result<T, ServiceError>;

pub fn validation<T>(msg: impl Into<String>) -> SResult<T> {
    Err(ServiceError::Validation(msg.into()))
}

pub fn not_found<T>(msg: impl Into<String>) -> SResult<T> {
    Err(ServiceError::NotFound(msg.into()))
}

pub fn conflict<T>(msg: impl Into<String>) -> SResult<T> {
    Err(ServiceError::Conflict(msg.into()))
}
