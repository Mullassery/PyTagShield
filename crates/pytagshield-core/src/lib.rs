//! PyTagShield core data plane.
//!
//! This crate contains the latency-sensitive, side-effect-light stages of the
//! tagging pipeline: ingestion validation, normalization, consent evaluation,
//! data minimization, deterministic transformations, routing, and the bounded
//! delivery workers that dispatch approved events to destinations.
//!
//! The crate is intentionally free of any Python dependency; Python bindings
//! live in the optional `pytagshield-python` crate.

pub mod attribution;
pub mod config;
pub mod consent;
pub mod delivery;
pub mod destination;
pub mod error;
pub mod event;
pub mod metrics;
pub mod minimize;
pub mod pipeline;
pub mod route;
pub mod transform;

pub use attribution::AttributionReport;
pub use config::Config;
pub use consent::{ConsentSnapshot, ConsentStore};
pub use delivery::{DeliveryLog, DeliveryRecord, DeliveryStatus, DeliveryTask};
pub use error::{PipelineError, ValidationError};
pub use event::{Event, EventContext};
pub use metrics::Metrics;
pub use pipeline::{IngestOutcome, Pipeline, ProcessedEvent};
