//! vyges-extract — foundry-correlated RC parasitic extraction engine.
//!
//! Turns a routed layout (DEF geometry) + a per-layer RC rules deck into a
//! SPEF (`*.spef`) parasitic model: for each signal net, accumulate per-layer
//! wirelength from the routing, apply resistance/capacitance rules, and emit
//! the IEEE-1481 `*D_NET` records the timing chain consumes.
//!
//! Boundaries (per the Vyges flow architecture): inputs and outputs are files
//! (DEF + RC rules in, SPEF out). The v0 model is a pure-std *rule-based*
//! extractor — analytic R/C from geometry, no subprocess — so the whole engine
//! (DEF parse, RC model, SPEF emit) is exercised offline and unit-tested. The
//! *correlated* upgrade (field-solve coupling caps, fit vs golden patterns) is
//! the part that will shell out to the EDA environment; the engine error type
//! already reserves that path (`ExtractError::FieldSolverNotFound`).

pub mod job;
pub mod rules;
pub mod def;
pub mod rc;
pub mod spef;
pub mod engine;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
