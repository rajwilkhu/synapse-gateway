//! Configurable input guardrails backed by `llm-guard`.
//! Per-route policy selection with fallback to a `default` policy.
pub mod engine;
pub mod policy;
pub mod scanners;
pub mod segments;
pub mod validate;

pub use engine::{GuardEngine, GuardLane};
pub use policy::GuardrailsConfig;
pub use validate::validate_guardrails;
