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

// The crate's own name inside it: the command-line entry ([`cli_main`]) is built into the
// library too, and its `vyges_extract::…` paths resolve here.
extern crate self as vyges_extract;

pub mod job;
pub mod rules;
// DEF + tech-LEF readers now come from the shared vyges-loom foundation. loom's
// Def is a superset (signal nets in µm for extraction + power/components for PDN);
// the signal `nets`/`DefNet`/`Segment` shapes extraction uses are unchanged.
pub use vyges_loom::{def, lef};
// Optional `gds -> DefNet` connectivity-tracing front-end: a GDS-only analog flow
// (no routed DEF). Sits ABOVE rc.rs — it produces `DefNet`s the unchanged RC core
// consumes. Uses the shared vyges-layout GDS kernel.
pub mod coupling;
/// The SPEF `*DATE` stamp and where its value comes from.
pub mod date;
pub mod engine;
pub mod field;
pub mod gds;
/// Std-cell pin/device hookup resolution (DEF + cell LEF + liberty → direction +
/// Cin), shared by the native DEF path and the KLayout front end.
pub mod hookup;
/// KLayout → SPEF front end: drive a headless-KLayout GDS net/geometry dumper and
/// render SPEF + the loom EM geometry sidecar. Parsing/serialization live in loom.
pub mod klayout;
pub mod rc;
pub mod spef;
pub mod tree;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COPYRIGHT: &str = "© 2026 Vyges. All Rights Reserved.  https://vyges.com";

/// The `vyges-extract` command-line entry (the binary's `main`), built into the library too so a
/// suite binary can link every engine and run the one it is invoked as.
#[cfg(feature = "suite")]
#[doc(hidden)]
#[path = "main.rs"]
pub mod cli_main;
