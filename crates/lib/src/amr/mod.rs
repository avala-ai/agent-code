//! # Agentic MapReduce (`amr`)
//!
//! A whole-repo analysis engine for tasks where an answer is only
//! trustworthy if the *entire* codebase was considered — security
//! scanning, breaking-change detection, large-scale migration. A single
//! search-driven agent spends most of its budget *finding* the work and
//! never proves it covered everything. AMR inverts that: an agent spends
//! reasoning once to author a *deterministic* relevance test, that test
//! runs over the whole tree with no model in the loop, and expensive
//! per-shard reasoning only touches files that actually matched.
//!
//! ```text
//!   repo ─▶ PLAN ──▶ SHARD ────▶ BATCH ──▶ MAP ─────▶ REDUCE ──▶ report
//!         (agent)  (determ.)   (determ.) (N agents)  (1 agent)
//!            │        │            │         │            │
//!    selectors    Signal[]     Batch[]   Finding[]   deduped +
//!    (cached,     drop zero-             per shard   chained +
//!     editable)   signal files                      prioritized
//! ```
//!
//! Only the stages that need judgement are agentic (PLAN authors the
//! decomposition, MAP investigates a shard, REDUCE synthesises). SHARD
//! and BATCH are pure, deterministic, and cacheable, so reruns pay only
//! for the diff.
//!
//! The first shipped application is security scanning (see [`profile`]);
//! the engine itself never hard-codes "vulnerability" — it orchestrates
//! `signals → shards → findings → chained findings`.
//!
//! ## Design notes
//!
//! - **Provider-agnostic.** MAP/REDUCE workers are in-process
//!   [`QueryEngine`](crate::query::QueryEngine) runs behind the
//!   [`agent::AmrAgent`] trait, so any configured LLM works and the
//!   orchestrator stays unit-testable with a fake agent.
//! - **Read-only workers.** MAP workers see only `FileRead`, `Grep`,
//!   `Glob` (enforced by the tool visibility filter), so a scan can
//!   never mutate the repo it is analysing.
//! - **Structured output by prompt, not by wire feature.** Workers are
//!   asked for a fenced JSON block and parsed defensively — portable
//!   across every provider rather than depending on one vendor's
//!   structured-output beta.

pub mod agent;
pub mod batch;
pub mod cache;
pub mod orchestrator;
pub mod profile;
pub mod reduce;
pub mod report;
pub mod selectors;
pub mod shard;
pub mod types;

pub use orchestrator::{ScanConfig, run_scan};
pub use types::{
    AttackChain, Batch, Finding, ScanReport, Severity, Signal, TokenTotals, WorkerFindings,
};

/// Errors surfaced by the AMR engine.
#[derive(Debug, thiserror::Error)]
pub enum AmrError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A selector, shard, or config was malformed.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A map/reduce worker could not produce a usable result.
    #[error("worker error: {0}")]
    Worker(String),
    /// A worker returned text that did not contain parseable findings.
    #[error("could not parse worker output: {0}")]
    Parse(String),
}
