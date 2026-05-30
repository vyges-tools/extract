//! Extraction job: the declarative description of what to extract.
//!
//! An `.ext` job is a tiny `key: value` file (std-only parser — no deps):
//!
//! ```text
//! design:   counter
//! def:      counter.def           # routed layout (geometry source)
//! rules:    sky130.rules          # per-layer RC coefficients
//! corner:   typical               # naming only in v0 (rules carry the values)
//! temp:     25
//! ```
//!
//! Paths are resolved relative to the job file's directory (`base_dir`), like
//! the characterization job. `corner`/`temp` are recorded for the SPEF header
//! and provenance; the actual R/C numbers live in the `rules` deck.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ExtractJob {
    pub design: String,
    pub def: String,
    pub rules: String,
    pub corner: String,
    pub temp: f64,
    pub base_dir: String,
}

#[derive(Debug)]
pub struct JobError(pub String);

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job error: {}", self.0)
    }
}
impl std::error::Error for JobError {}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

impl ExtractJob {
    pub fn parse(text: &str, base_dir: &str) -> Result<ExtractJob, JobError> {
        let mut kv: BTreeMap<String, String> = BTreeMap::new();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once(':')
                .ok_or_else(|| JobError(format!("expected 'key: value', got {line:?}")))?;
            kv.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
        let get = |k: &str| -> Result<String, JobError> {
            kv.get(k).cloned().ok_or_else(|| JobError(format!("missing key: {k}")))
        };
        let job = ExtractJob {
            design: get("design")?,
            def: get("def")?,
            rules: get("rules")?,
            corner: kv.get("corner").cloned().unwrap_or_else(|| "typical".into()),
            temp: kv.get("temp").and_then(|t| t.parse().ok()).unwrap_or(25.0),
            base_dir: base_dir.to_string(),
        };
        job.validate()?;
        Ok(job)
    }

    pub fn load(path: &str) -> Result<ExtractJob, JobError> {
        let text = std::fs::read_to_string(path).map_err(|e| JobError(format!("{path}: {e}")))?;
        let base = Path::new(path).parent().and_then(|p| p.to_str()).unwrap_or(".");
        ExtractJob::parse(&text, base)
    }

    /// A job-relative path resolved against `base_dir`.
    pub fn resolve(&self, rel: &str) -> String {
        if Path::new(rel).is_absolute() || self.base_dir.is_empty() {
            rel.to_string()
        } else {
            Path::new(&self.base_dir).join(rel).to_string_lossy().into_owned()
        }
    }

    pub fn validate(&self) -> Result<(), JobError> {
        if self.design.is_empty() {
            return Err(JobError("design is required".into()));
        }
        if self.def.is_empty() || self.rules.is_empty() {
            return Err(JobError("def and rules are required".into()));
        }
        Ok(())
    }
}
