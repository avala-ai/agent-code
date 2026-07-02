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

/// Parse a MAP worker reply, distinguishing a usable findings envelope
/// (possibly empty) from output that never actually assessed the shard.
///
/// Returns `None` when the reply is not a usable envelope — no JSON at all,
/// valid JSON of the wrong shape (`{"error": ...}`), or a `findings` array
/// whose elements fail to deserialize — which the orchestrator treats as
/// incomplete coverage rather than a covered-but-clean shard.
pub fn parse_worker_findings_checked(text: &str) -> Option<Vec<Finding>> {
    let v = extract_json_value(text)?;
    let arr = if v.is_array() {
        v.as_array()
    } else {
        v.get("findings").and_then(|f| f.as_array())
    }?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        // A single element that fails to deserialize means the shard's output
        // is untrustworthy — surface it as incomplete rather than dropping it.
        out.push(serde_json::from_value::<Finding>(item.clone()).ok()?);
    }
    Some(out)
}

/// Parse every element of a findings array strictly. Returns `None` if any
/// element fails to deserialize, so callers can distinguish "the model listed
/// findings we could parse" from "the model listed findings but the output is
/// malformed" — the latter must not be silently reduced to a shorter (or
/// empty) list.
fn strict_findings(items: &[Value]) -> Option<Vec<Finding>> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(serde_json::from_value::<Finding>(it.clone()).ok()?);
    }
    Some(out)
}

/// Parse every element of a chains array strictly. Returns `None` if any
/// element fails to deserialize, so a malformed chain marks the reduce output
/// untrusted rather than being silently dropped (which could hide a chain-only
/// over-threshold result behind a clean exit).
fn strict_chains(items: &[Value]) -> Option<Vec<AttackChain>> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(serde_json::from_value::<AttackChain>(it.clone()).ok()?);
    }
    Some(out)
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
    parse_reduce_result_checked(text, fallback).0
}

/// Like [`parse_reduce_result`], but also reports whether the reply was a
/// *trustworthy* reduce envelope.
///
/// `trusted` is false when the reducer ran but produced no usable envelope —
/// no JSON, valid JSON of the wrong shape (`{"error": ...}`), a truncated/prose
/// turn (e.g. a turn-cap or budget stop returns `Ok` with unparseable text), or
/// a findings array whose elements fail to deserialize. In every such case the
/// reducer's cross-shard **chain** composition (which nothing else produces)
/// cannot be relied on, so the caller must treat coverage as incomplete rather
/// than reporting a false clean. `result` still carries the best-effort
/// findings (the reducer's when usable, else the MAP fallback) so nothing is
/// silently dropped, and `chains` are only surfaced when the envelope parsed
/// cleanly.
pub fn parse_reduce_result_checked(text: &str, fallback: &[Finding]) -> (ReduceResult, bool) {
    let untrusted = || {
        (
            ReduceResult {
                findings: fallback.to_vec(),
                chains: Vec::new(),
            },
            false,
        )
    };

    let Some(v) = extract_json_value(text) else {
        return untrusted();
    };
    if !(v.is_array() || v.get("findings").is_some()) {
        // Valid JSON but not a findings envelope (wrong shape).
        return untrusted();
    }

    let arr = if v.is_array() {
        v.as_array()
    } else {
        v.get("findings").and_then(|f| f.as_array())
    };
    let (findings, trusted) = match arr {
        // An explicit empty array is a deliberate "all candidates were false
        // positives" verdict — honor it, and it IS a trustworthy envelope.
        Some(items) if items.is_empty() => (Vec::new(), true),
        // A non-empty array is trustworthy only if EVERY element deserializes;
        // otherwise the output is malformed (schema drift, truncation) — fall
        // back to the MAP findings AND flag it untrusted so the gate reports
        // incomplete coverage instead of erasing real work.
        Some(items) => match strict_findings(items) {
            Some(parsed) => (parsed, true),
            None => (fallback.to_vec(), false),
        },
        // `findings` present but not an array (null, string, object).
        None => (fallback.to_vec(), false),
    };

    // Chains are only believable when the envelope parsed cleanly, and they are
    // parsed strictly too: a malformed chain (like a malformed finding) taints
    // the whole reduce output. Silently dropping it could hide a chain-only
    // P0/P1 — a chain can be over-threshold even when its members are not — and
    // flip the gate to a clean exit, so mark the result untrusted instead.
    let (chains, trusted) = if !trusted {
        (Vec::new(), false)
    } else {
        match v.get("chains") {
            // No chains field is a valid "no cross-shard chains" result.
            None => (Vec::new(), true),
            Some(c) => match c.as_array() {
                Some(items) => match strict_chains(items) {
                    Some(parsed) => (parsed, true),
                    None => (Vec::new(), false),
                },
                // `chains` present but not an array: unusable envelope.
                None => (Vec::new(), false),
            },
        }
    };

    (ReduceResult { findings, chains }, trusted)
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
    fn checked_parse_distinguishes_envelope_from_garbage() {
        // No JSON and wrong-shape JSON both mean the shard was not assessed.
        assert!(parse_worker_findings_checked("i refuse").is_none());
        assert!(parse_worker_findings_checked(r#"{"error":"rate limited"}"#).is_none());
        // A valid but empty envelope is a covered shard with no findings.
        assert_eq!(
            parse_worker_findings_checked(r#"{"findings":[]}"#).map(|v| v.len()),
            Some(0)
        );
        // A well-formed finding parses.
        let ok = r#"{"findings":[{"id":"f","file":"a.py","severity":"P0","title":"t"}]}"#;
        assert_eq!(parse_worker_findings_checked(ok).map(|v| v.len()), Some(1));
        // A finding missing a required field taints the whole shard.
        let bad = r#"{"findings":[{"id":"f","file":"a.py","title":"no severity"}]}"#;
        assert!(parse_worker_findings_checked(bad).is_none());
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
    fn reduce_checked_reports_trust_correctly() {
        let fb = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        // Trusted: a clean envelope with valid findings.
        let (_r, t) = parse_reduce_result_checked(
            "```json\n{\"findings\":[{\"id\":\"g\",\"file\":\"a.py\",\"severity\":\"P0\",\"title\":\"x\"}],\"chains\":[]}\n```",
            &fb,
        );
        assert!(t, "valid envelope is trusted");
        // Trusted: an explicit empty verdict.
        let (_r, t) =
            parse_reduce_result_checked("```json\n{\"findings\":[],\"chains\":[]}\n```", &fb);
        assert!(t, "explicit empty is trusted");
        // Untrusted: a turn-cap / prose reply with no JSON — chains lost.
        let (r, t) = parse_reduce_result_checked("I ran out of turns before writing JSON.", &fb);
        assert!(!t, "prose / no-JSON is untrusted");
        assert_eq!(r.findings.len(), 1, "MAP fallback still surfaced");
        assert!(r.chains.is_empty());
        // Untrusted: wrong-shape JSON.
        let (_r, t) = parse_reduce_result_checked("{\"error\":\"rate limited\"}", &fb);
        assert!(!t, "wrong-shape JSON is untrusted");
        // Untrusted: malformed findings element (drops trust, keeps fallback).
        let (r, t) =
            parse_reduce_result_checked("```json\n{\"findings\":[{\"id\":\"f\"}]}\n```", &fb);
        assert!(!t, "malformed findings are untrusted");
        assert_eq!(r.findings.len(), 1, "MAP fallback preserved");
    }

    #[test]
    fn reduce_malformed_chain_marks_output_untrusted() {
        // Findings parse fine, but a chain element is malformed (missing
        // required fields). The chain must not be silently dropped while the
        // result stays trusted — that could hide a chain-only over-threshold
        // result behind a clean exit. Mark it untrusted so the gate flags
        // incomplete coverage.
        let fb = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        let text = "```json\n{\"findings\":[\
            {\"id\":\"g\",\"file\":\"a.py\",\"severity\":\"P0\",\"title\":\"ok\"}],\
            \"chains\":[{\"chain_id\":\"c1\"}]}\n```";
        let (result, trusted) = parse_reduce_result_checked(text, &fb);
        assert!(!trusted, "a malformed chain taints the reduce output");
        assert!(
            result.chains.is_empty(),
            "the malformed chain is not surfaced"
        );
        // A well-formed chains array (or none) stays trusted.
        let (_r, t) =
            parse_reduce_result_checked("```json\n{\"findings\":[],\"chains\":[]}\n```", &fb);
        assert!(t, "empty chains + empty findings is a trusted verdict");
    }

    #[test]
    fn reduce_malformed_findings_fall_back_instead_of_erasing() {
        // The reducer returns a NON-empty findings array, but the sole element
        // is missing required fields (schema drift / truncation). This must not
        // erase the MAP P0 — it falls back to the MAP findings so the CI gate
        // still fires.
        let fallback = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        let result = parse_reduce_result(
            "```json\n{\"findings\":[{\"id\":\"f-0000\"}],\"chains\":[]}\n```",
            &fallback,
        );
        assert_eq!(result.findings.len(), 1, "MAP finding must survive");
        assert_eq!(result.findings[0].severity, Severity::P0);
    }

    #[test]
    fn reduce_partial_parse_failure_falls_back() {
        // One good finding, one malformed: the whole reduce output is
        // untrusted, so we keep the MAP findings rather than a lossy subset.
        let fallback = vec![
            finding("f1", "a.py", Severity::P0, 0.9),
            finding("f2", "b.py", Severity::P1, 0.8),
        ];
        let text = "```json\n{\"findings\":[\
            {\"id\":\"g\",\"file\":\"a.py\",\"severity\":\"P0\",\"title\":\"ok\"},\
            {\"id\":\"bad\",\"title\":\"missing file+severity\"}],\"chains\":[]}\n```";
        let result = parse_reduce_result(text, &fallback);
        assert_eq!(result.findings.len(), 2, "falls back to both MAP findings");
    }

    #[test]
    fn reduce_non_array_findings_field_falls_back() {
        // `findings` present but not an array is unusable → keep MAP findings.
        let fallback = vec![finding("f1", "a.py", Severity::P0, 0.9)];
        let result = parse_reduce_result("```json\n{\"findings\": null}\n```", &fallback);
        assert_eq!(result.findings.len(), 1);
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
