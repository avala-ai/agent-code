//! Scan profiles: the task-specific half of AMR.
//!
//! A [`Profile`] bundles the deterministic selectors that decide which
//! code is relevant with the prompt preambles that tell MAP and REDUCE
//! workers what to look for. The engine itself is task-agnostic; swapping
//! the profile is what turns "find vulnerabilities" into "find dead code"
//! or "find breaking changes".
//!
//! The vertical slice ships one built-in profile, [`security_profile`].
//! Its selectors are a recall-oriented net: being a little broad is fine
//! because the MAP worker applies a false-positive gate. Additional
//! profiles and user-authored (TOML) profiles are future work.

use regex::Regex;

use super::selectors::{Lang, Selector, SelectorKind};

/// A named analysis profile.
pub struct Profile {
    pub name: &'static str,
    pub description: &'static str,
    /// Deterministic relevance tests run over the whole tree.
    pub selectors: Vec<Selector>,
    /// Instructions prepended to each MAP worker prompt.
    pub investigate_preamble: &'static str,
    /// Instructions prepended to the REDUCE prompt.
    pub reduce_preamble: &'static str,
    /// Human-readable severity rubric, echoed into prompts.
    pub severity_rubric: &'static str,
}

fn lex(id: &str, description: &str, langs: Vec<Lang>, re: &str) -> Selector {
    Selector {
        id: id.to_string(),
        description: description.to_string(),
        langs,
        kind: SelectorKind::Lexical {
            pattern: Regex::new(re).expect("built-in selector regex must compile"),
        },
    }
}

fn ast_call(
    id: &str,
    description: &str,
    langs: Vec<Lang>,
    node_kinds: &[&str],
    callee: &str,
) -> Selector {
    Selector {
        id: id.to_string(),
        description: description.to_string(),
        langs,
        kind: SelectorKind::Ast {
            node_kinds: node_kinds.iter().map(|s| s.to_string()).collect(),
            callee: Some(Regex::new(callee).expect("built-in selector regex must compile")),
        },
    }
}

/// The built-in security profile: whole-repo vulnerability discovery.
pub fn security_profile() -> Profile {
    let selectors = vec![
        // --- Remote code execution / command injection -----------------
        ast_call(
            "rce.py_dangerous_call",
            "Python call to a code/command execution sink",
            vec![Lang::Python],
            &["call"],
            r"^(os\.system|os\.popen|subprocess\.(Popen|call|run|check_output|check_call)|eval|exec|__import__|compile)\b",
        ),
        ast_call(
            "rce.js_dangerous_call",
            "JavaScript call to a code/command execution sink",
            vec![Lang::JavaScript],
            &["call_expression"],
            r"(child_process|\.exec(Sync)?\s*\(|\beval\s*\(|new Function|vm\.runIn)",
        ),
        lex(
            "rce.subprocess_shell_true",
            "subprocess invoked with shell=True",
            vec![Lang::Python],
            r"shell\s*=\s*True",
        ),
        lex(
            "rce.node_exec",
            "Node child_process exec sink",
            vec![Lang::JavaScript, Lang::TypeScript],
            r"child_process|\bexecSync\b|\bexec\s*\(",
        ),
        // --- Insecure deserialization ----------------------------------
        lex(
            "deser.python",
            "Insecure deserialization sink",
            vec![Lang::Python],
            r"(pickle\s*\.\s*loads?|marshal\s*\.\s*loads?|yaml\s*\.\s*load\s*\(|jsonpickle)",
        ),
        // --- SQL injection ---------------------------------------------
        lex(
            "sqli.raw_query",
            "Raw SQL execution with possible string building",
            vec![
                Lang::Python,
                Lang::JavaScript,
                Lang::TypeScript,
                Lang::Go,
                Lang::Java,
                Lang::Ruby,
                Lang::Php,
            ],
            r"(execute|executemany|executescript|query|raw)\s*\(",
        ),
        lex(
            "sqli.fstring",
            "SQL keyword inside an interpolated/f-string",
            vec![],
            r#"(?i)(select|insert into|update|delete from)\b[^;\n]*(\{|%s|\$\{|"\s*\+|'\s*\+|f")"#,
        ),
        // --- Server-side request forgery -------------------------------
        lex(
            "ssrf.http_client",
            "Outbound HTTP request (possible SSRF sink)",
            vec![Lang::Python, Lang::JavaScript, Lang::TypeScript],
            r"(requests\.(get|post|put|delete|head|request)|urllib\.request\.urlopen|axios\.|fetch\s*\(|http\.get)",
        ),
        // --- Path traversal --------------------------------------------
        lex(
            "path.open_concat",
            "File open with a concatenated/interpolated path",
            vec![Lang::Python, Lang::JavaScript, Lang::TypeScript],
            r#"(open|readFile|readFileSync|sendFile|createReadStream)\s*\([^)\n]*(\+|\{|\$\{|%s|os\.path\.join)"#,
        ),
        // --- Template injection ----------------------------------------
        lex(
            "ssti.render_string",
            "Template rendered from a string (possible SSTI)",
            vec![Lang::Python],
            r"render_template_string\s*\(",
        ),
        // --- Weak cryptography -----------------------------------------
        lex(
            "crypto.weak_hash",
            "Weak hash primitive",
            vec![
                Lang::Python,
                Lang::JavaScript,
                Lang::TypeScript,
                Lang::Go,
                Lang::Java,
            ],
            r"(?i)\b(md5|sha1)\b\s*[\(:]",
        ),
        // --- Hardcoded secrets -----------------------------------------
        lex(
            "secret.aws_key",
            "AWS access key id literal",
            vec![],
            r"AKIA[0-9A-Z]{16}",
        ),
        lex(
            "secret.private_key",
            "Embedded private key",
            vec![],
            r"-----BEGIN (RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----",
        ),
        lex(
            "secret.assignment",
            "Credential-like literal assignment",
            vec![],
            r#"(?i)(password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token)\s*[:=]\s*["'][^"'\n]{6,}["']"#,
        ),
        // --- Broad recall safety net -----------------------------------
        lex(
            "auth.decorator",
            "Auth/permission boundary marker",
            vec![Lang::Python],
            r"(?i)@(login_required|permission_required|requires_auth|authenticated)",
        ),
    ];

    Profile {
        name: "security",
        description: "Find exploitable vulnerabilities across the whole repository via Agentic MapReduce.",
        selectors,
        investigate_preamble: INVESTIGATE_PREAMBLE,
        reduce_preamble: REDUCE_PREAMBLE,
        severity_rubric: SEVERITY_RUBRIC,
    }
}

const SEVERITY_RUBRIC: &str = "\
P0 = remotely exploitable, no authentication, integrity/confidentiality loss.
P1 = exploitable with preconditions or authentication.
P2 = requires local access or unusual configuration.";

const INVESTIGATE_PREAMBLE: &str = "\
You are a security investigator examining ONE shard of a larger codebase.
You are given only the files and signals for this shard. Do not assume
anything about the rest of the repository.

For every candidate signal:
  1. Read the real code around it with the read-only tools you have.
  2. Decide whether a genuine, exploitable vulnerability exists.
  3. Apply the false-positive gate: if you cannot articulate a concrete
     exploit path and its preconditions, DO NOT report it.

Work efficiently and stay in scope. Read the files named in the signals and
the immediate code around each signal; you may open a directly imported
helper when it is essential, but do NOT crawl the wider repository or run
broad searches. As soon as you have assessed this shard's signals, output
your JSON and stop.

Account for every file you were handed. Prefer a small number of real,
well-evidenced findings over many speculative ones.";

const REDUCE_PREAMBLE: &str = "\
You are given deduplicated candidate findings from many independent shard
workers (their conclusions only, not their transcripts, and not the whole
repository). Your job:
  1. Deduplicate findings that describe the same root cause in the same place.
  2. Reconcile conflicting severities and keep the best-evidenced verdict.
  3. Compose ATTACK CHAINS: sequences where lower-severity findings combine
     into a higher-impact one (e.g. an unauthenticated ID disclosure plus an
     ID-gated RCE becomes one P0 unauthenticated RCE).
  4. Produce a single prioritised list, most severe first.
Only reason over the findings provided; do not invent new ones.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn security_profile_selectors_all_compile_and_have_unique_ids() {
        let p = security_profile();
        assert!(!p.selectors.is_empty());
        let mut ids: Vec<_> = p.selectors.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "selector ids must be unique");
    }

    #[test]
    fn security_profile_flags_a_classic_python_rce() {
        let p = security_profile();
        let text = "import os\ndef handler(req):\n    os.system('ping ' + req.args['host'])\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("views.py"), text))
            .collect();
        assert!(
            signals
                .iter()
                .any(|s| s.selector_id == "rce.py_dangerous_call"),
            "os.system call should produce a signal, got {signals:?}"
        );
    }

    #[test]
    fn security_profile_flags_a_hardcoded_secret() {
        let p = security_profile();
        let text = "const config = { api_key: \"sk-supersecretvalue123\" }\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("config.js"), text))
            .collect();
        assert!(signals.iter().any(|s| s.selector_id == "secret.assignment"));
    }

    #[test]
    fn security_profile_ignores_clean_code() {
        let p = security_profile();
        let text = "def add(a, b):\n    return a + b\n";
        let signals: Vec<_> = p
            .selectors
            .iter()
            .flat_map(|s| s.scan_text(Path::new("math_utils.py"), text))
            .collect();
        assert!(
            signals.is_empty(),
            "benign code should emit no signals, got {signals:?}"
        );
    }
}
