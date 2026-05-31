//! Minimal LEF reader — per-layer default routing width.
//!
//! Extraction only needs one thing from LEF for edge-gap coupling: the default
//! routing **WIDTH** of each layer. v1 reads exactly that — the `LAYER <name> …
//! WIDTH <w> ; … END <name>` blocks — and ignores everything else (vias, macros,
//! pins). Widths let coupling use the true edge-to-edge gap rather than the
//! centerline distance. Pure std — fully unit-tested offline.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Lef {
    pub widths: BTreeMap<String, f64>, // layer -> default routing width (um)
}

#[derive(Debug)]
pub struct LefError(pub String);
impl std::fmt::Display for LefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lef error: {}", self.0)
    }
}
impl std::error::Error for LefError {}

impl Lef {
    pub fn parse(text: &str) -> Lef {
        let mut widths = BTreeMap::new();
        let mut cur: Option<String> = None; // layer currently open
        let mut width: Option<f64> = None;
        for raw in text.lines() {
            let line = match raw.find('#') {
                Some(i) => &raw[..i],
                None => raw,
            };
            let toks: Vec<&str> = line.split_whitespace().collect();
            match toks.as_slice() {
                ["LAYER", name, ..] => {
                    cur = Some(name.to_string());
                    width = None;
                }
                ["WIDTH", w, ..] => {
                    width = w.trim_end_matches(';').parse::<f64>().ok();
                }
                ["END", name, ..] if cur.as_deref() == Some(*name) => {
                    if let (Some(l), Some(w)) = (cur.take(), width.take()) {
                        widths.insert(l, w);
                    }
                }
                _ => {}
            }
        }
        Lef { widths }
    }

    pub fn load(path: &str) -> Result<Lef, LefError> {
        let text = std::fs::read_to_string(path).map_err(|e| LefError(format!("{path}: {e}")))?;
        Ok(Lef::parse(&text))
    }

    /// Default routing width for a layer (0.0 if unknown → falls back to a
    /// centerline gap in coupling).
    pub fn width(&self, layer: &str) -> f64 {
        self.widths.get(layer).copied().unwrap_or(0.0)
    }
}
