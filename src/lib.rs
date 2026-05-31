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
//! (DEF parse, RC model, coupling, SPEF emit) is exercised offline and
//! unit-tested. v1 adds **lateral coupling capacitance** from segment adjacency
//! (`coupling`); the field-solved 2.5-D kernel + golden-pattern fit (M5) replace
//! that model behind the same SPEF output (`ExtractError::FieldSolverNotFound`
//! reserves the shell-out path).

pub mod job;
pub mod rules;
pub mod def;
pub mod lef;
pub mod rc;
pub mod coupling;
pub mod spef;
pub mod engine;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
