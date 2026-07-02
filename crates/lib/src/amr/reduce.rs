//! REDUCE stage helpers: parse worker output, dedup, and drive the reducer.
//!
//! Worker replies are free-form model text, so JSON is extracted
//! defensively (a fenced block if present, otherwise the first balanced
//! brace span, otherwise the whole reply). One malformed finding never
//! discards the rest — findings are parsed element by element. A cheap,
//! deterministic dedup pre-pass shrinks the set before the REDUCE agent
//! reasons over it.

use std::collections::BTreeSet;

use serde_json::Value;

use super::profile::Profile;
use super::types::{AttackChain, Finding, ReduceResult, Severity, WorkerFindings};

/// Extract the most likely JSON value embedded in model text.
pub fn extract_json_value(text: &str) -> Option<Value> {
    if let Some(block) = fenced_block(text)
        && let Ok(v) = serde_json::from_str::<Value>(block.trim())
    {
        return Some(v);
    }
    if let Some(span) = balanced_span(text)
        && let Ok(v) = serde_json::from_str::<Value>(span)
    {
        return Some(v);
    }
    serde_json::from_str::<Value>(text.trim()).ok()
}

/// Contents of the first fenced code block (```json … ``` or plain ```).
fn fenced_block(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    // Skip an optional language tag up to the first newline.
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &after[body_start..];
    let end = body.find("```")?;
    Some(&body[..end])
}

/// The first balanced `{…}` or `[…]` span, ignoring braces inside strings.
fn balanced_span(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{' || b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };

    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            x if x == open => depth += 1,
            x if x == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a MAP worker's reply into findings. Never fails: an unparseable
/// reply (or a worker that reported nothing) yields an empty set.
pub fn parse_worker_findings(text: &str) -> WorkerFindings {
    match extract_json_value(text) {
        Some(v) => WorkerFindings {
            findings: findings_from_value(&v),
        },
        None => WorkerFindings::default(),
    }
}

fn findings_from_value(v: &Value) -> Vec<Finding> {
    let arr = if v.is_array() {
        v.as_array()
    } else {
        v.get("findings").and_then(|f| f.as_array())
    };
    match arr {
        Some(items) => items
            .iter()
            .filter_map(|it| serde_json::from_value::<Finding>(it.clone()).ok())
            .collect(),
        None => Vec::new(),
    }
}

/// Deterministic dedup pre-pass. Collapses findings that describe the same
/// issue in the same place (same CWE, file, nearby lines, and title), so
/// the REDUCE agent sees a smaller, cleaner set. Order is preserved.
pub fn dedup_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in findings {
        let title_key: String = f.title.to_lowercase().chars().take(40).collect();
        let line_bucket = f.line_range.map(|(a, _)| a / 5).unwrap_or(0);
        let key = format!(
            "{}|{}|{}|{}",
            f.cwe.as_deref().unwrap_or("").to_lowercase(),
            f.file.to_lowercase(),
            line_bucket,
            title_key
        );
        if seen.insert(key) {
            out.push(f);
        }
    }
    out
}

/// Backfill deterministic ids for any finding a worker left un-ided.
pub fn assign_ids(findings: &mut [Finding]) {
    for (i, f) in findings.iter_mut().enumerate() {
        if f.id.trim().is_empty() {
            f.id = format!("f-{i:04}");
        }
    }
}

/// Sort findings most-severe first, then by confidence, deterministically.
pub fn prioritize(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.severity
            .rank()
            .cmp(&a.severity.rank())
            .then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.file.cmp(&b.file))
            .then(a.id.cmp(&b.id))
    });
}

/// Build the REDUCE prompt: preamble, rubric, the finding set, and a strict
/// output-format instruction.
pub fn build_reduce_prompt(profile: &Profile, findings: &[Finding]) -> String {
    let findings_json = serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".to_string());
    format!(
        "{preamble}\n\nSEVERITY RUBRIC:\n{rubric}\n\nCANDIDATE FINDINGS (JSON):\n{findings}\n\n\
Respond with ONLY a single fenced ```json block matching this schema:\n\
{{\"findings\": [ {{\"id\": string, \"cwe\": string|null, \"file\": string, \
\"line_range\": [int,int]|null, \"severity\": \"P0\"|\"P1\"|\"P2\", \"confidence\": number, \
\"title\": string, \"root_cause\": string, \"exploit_preconditions\": string, \"evidence\": string}} ], \
\"chains\": [ {{\"chain_id\": string, \"member_finding_ids\": [string], \
\"combined_severity\": \"P0\"|\"P1\"|\"P2\", \"narrative\": string, \"combined_preconditions\": string}} ]}}\n\
Keep every real finding (deduplicated and reprioritised). Return an empty chains array if there are no cross-shard chains.",
        preamble = profile.reduce_preamble,
        rubric = profile.severity_rubric,
        findings = findings_json,
    )
}

/// Parse the reducer's reply. Falls back to the deduped input findings (no
/// chains) if the reply is unparseable, so a flaky reducer never loses the
/// work MAP already did.
pub fn parse_reduce_result(text: &str, fallback: &[Finding]) -> ReduceResult {
    let Some(v) = extract_json_value(text) else {
        return ReduceResult {
            findings: fallback.to_vec(),
            chains: Vec::new(),
        };
    };

    let findings = if v.is_array() || v.get("findings").is_some() {
        // The reducer returned an explicit findings list. An empty list is a
        // deliberate verdict — it cleared every MAP candidate as a false
        // positive — so honor it rather than restoring the fallback.
        findings_from_value(&v)
    } else {
        // No findings field at all: the reply is unusable, so keep the MAP
        // findings rather than silently dropping real work.
        fallback.to_vec()
    };
    let chains = v
        .get("chains")
        .and_then(|c| c.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|it| serde_json::from_value::<AttackChain>(it.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    ReduceResult { findings, chains }
}

/// Findings at or above `threshold`.
pub fn filter_by_severity(findings: Vec<Finding>, threshold: Severity) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| f.severity.rank() >= threshold.rank())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, file: &str, sev: Severity, conf: f64) -> Finding {
        Finding {
            id: id.into(),
            cwe: Some("CWE-89".into()),
            file: file.into(),
            line_range: Some((10, 12)),
            severity: sev,
            confidence: conf,
            title: "SQL injection".into(),
            root_cause: String::new(),
            exploit_preconditions: String::new(),
            evidence: String::new(),
            selector_id: None,
            shard_id: None,
        }
    }

    #[test]
    fn extracts_fenced_json() {
        let text = "Here is my answer:\n```json\n{\"findings\": []}\n```\nDone.";
        let v = extract_json_value(text).unwrap();
        assert!(v.get("findings").is_some());
    }

    #[test]
    fn extracts_balanced_span_without_fence() {
        let text = "sure: {\"findings\": [{\"id\":\"x\"}]} trailing text";
        let v = extract_json_value(text).unwrap();
        assert_eq!(v["findings"][0]["id"], "x");
    }

    #[test]
    fn balanced_span_ignores_braces_in_strings() {
        let text = r#"{"title": "watch out for } this", "n": 1}"#;
        let v = extract_json_value(text).unwrap();
        assert_eq!(v["n"], 1);
    }

    #[test]
    fn parse_worker_findings_tolerates_prose_and_bad_elements() {
        let text = "I found one issue.\n```json\n{\"findings\":[\
            {\"id\":\"f1\",\"file\":\"a.py\",\"severity\":\"P0\",\"title\":\"eval\"},\
            {\"garbage\":true}]}\n```";
        let wf = parse_worker_findings(text);
        // The malformed element is skipped, the valid one kept.
        assert_eq!(wf.findings.len(), 1);
        assert_eq!(wf.findings[0].severity, Severity::P0);
    }

    #[test]
    fn parse_worker_findings_on_garbage_is_empty() {
        assert!(
            parse_worker_findings("no json here at all")
                .findings
                .is_empty()
        );
    }

    #[test]
    fn parse_worker_findings_accepts_bare_array() {
        let text = r#"[{"id":"f1","file":"a.py","severity":"P1","title":"x"}]"#;
        assert_eq!(parse_worker_findings(text).findings.len(), 1);
    }

    #[test]
    fn dedup_collapses_same_issue_same_place() {
        let f = vec![
            finding("a", "app.py", Severity::P0, 0.9),
            finding("b", "app.py", Severity::P0, 0.7), // same place → dup
            finding("c", "other.py", Severity::P0, 0.9),
        ];
        let out = dedup_findings(f);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "a");
    }

    #[test]
    fn prioritize_orders_by_severity_then_confidence() {
        let mut f = vec![
            finding("low", "a.py", Severity::P2, 0.9),
            finding("high_lo_conf", "b.py", Severity::P0, 0.4),
            finding("high_hi_conf", "c.py", Severity::P0, 0.95),
        ];
        prioritize(&mut f);
        assert_eq!(f[0].id, "high_hi_conf");
        assert_eq!(f[1].id, "high_lo_conf");
        assert_eq!(f[2].id, "low");
    }

    #[test]
    fn assign_ids_backfills_missing() {
        let mut f = vec![finding("", "a.py", Severity::P1, 0.5)];
        assign_ids(&mut f);
        assert_eq!(f[0].id, "f-0000");
    }

    #[test]
    fn reduce_fallback_keeps_findings_when_reducer_output_unparseable() {
        let fallback = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        let result = parse_reduce_result("the reducer said nothing parseable", &fallback);
        assert_eq!(result.findings.len(), 1);
        assert!(result.chains.is_empty());
    }

    #[test]
    fn reduce_honors_explicit_empty_findings() {
        let fallback = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        // A valid, explicit empty list means the reducer rejected every
        // candidate as a false positive — it must not be restored.
        let result = parse_reduce_result(
            "```json\n{\"findings\": [], \"chains\": []}\n```",
            &fallback,
        );
        assert!(result.findings.is_empty());
    }

    #[test]
    fn reduce_parses_findings_and_chains() {
        let text = "```json\n{\"findings\":[{\"id\":\"f1\",\"file\":\"a.py\",\"severity\":\"P0\",\"title\":\"rce\"}],\
            \"chains\":[{\"chain_id\":\"c1\",\"member_finding_ids\":[\"f1\"],\"combined_severity\":\"P0\",\"narrative\":\"n\"}]}\n```";
        let result = parse_reduce_result(text, &[]);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.chains.len(), 1);
        assert_eq!(result.chains[0].combined_severity, Severity::P0);
    }

    #[test]
    fn severity_filter_gates() {
        let f = vec![
            finding("a", "x.py", Severity::P0, 0.9),
            finding("b", "y.py", Severity::P2, 0.9),
        ];
        assert_eq!(filter_by_severity(f, Severity::P1).len(), 1);
    }
}
