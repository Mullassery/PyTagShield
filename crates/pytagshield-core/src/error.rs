use serde::Serialize;

/// Structured validation failure for a single ingest request.
///
/// All accumulated problems are reported at once so callers can fix their
/// payloads in a single round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("event validation failed")]
pub struct ValidationError {
    pub errors: Vec<String>,
}

impl ValidationError {
    pub fn new(errors: Vec<String>) -> Self {
        Self { errors }
    }

    pub fn single(message: impl Into<String>) -> Self {
        Self {
            errors: vec![message.into()],
        }
    }
}

/// Errors produced while processing an event through the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error("configuration error: {0}")]
    Config(String),
}

impl PipelineError {
    pub fn is_validation(&self) -> bool {
        matches!(self, PipelineError::Validation(_))
    }
}
