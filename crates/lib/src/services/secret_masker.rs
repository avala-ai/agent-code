//! Secret masking for persistence boundaries.
//!
//! Redacts sensitive patterns (API keys, passwords, private keys)
//! from any string before it crosses a persistence boundary:
//! disk writes, LLM summarization prompts, transcript exports.
//!
//! This is defence-in-depth. Tool results, user messages, and file
//! contents can contain secrets that should never be written to
//! session files, compression summaries, or output-store dumps.
//!
//! Matched content is replaced with `[REDACTED:<kind>]`.

use regex::Regex;
use std::sync::LazyLock;

/// Ordered list of (kind, pattern). Order matters: specific providers
/// match before the generic key/value rule so the kind label is useful.
struct Pattern {
    kind: &'static str,
    re: Regex,
}

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let specs: &[(&str, &str)] = &[
        // PEM private keys — multiline, match whole block.
        (
            "private_key",
            r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----",
        ),
        // AWS access key ID.
        ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
        // GitHub personal access token.
        ("github_pat", r"ghp_[a-zA-Z0-9]{36}"),
        // GitHub fine-grained token.
        ("github_token", r"github_pat_[a-zA-Z0-9_]{50,}"),
        // Provider API keys (OpenAI, Anthropic etc.) — long opaque prefix.
        ("provider_api_key", r"sk-[a-zA-Z0-9_\-]{20,}"),
        // Generic assignment: api_key = "...", password: ..., auth_token=...
        // Captures the full identifier (group 1) and masks the value.
        // The identifier may contain the sensitive word anywhere inside it
        // (e.g. `my_api_key`, `auth_token`, `SESSION_SECRET`).
        (
            "credential",
            r#"(?i)\b((?:[a-z][a-z0-9_-]*[_-])?(?:api[_-]?key|secret|password|passwd|token|auth)[a-z0-9_-]*)\s*[:=]\s*["']?[A-Za-z0-9_\-./+=]{8,}["']?"#,
        ),
    ];

    specs
        .iter()
        .map(|(kind, pat)| Pattern {
            kind,
            re: Regex::new(pat).expect("secret_masker pattern must compile"),
        })
        .collect()
});

/// Mask all known secret patterns in `input`.
///
/// Idempotent: running twice produces the same output (the replacement
/// `[REDACTED:<kind>]` contains no characters that match any pattern).
pub fn mask(input: &str) -> String {
    let mut out = input.to_string();
    for p in PATTERNS.iter() {
        // For the generic credential rule, keep the key name visible.
        if p.kind == "credential" {
            out =
                p.re.replace_all(&out, "${1}=[REDACTED:credential]")
                    .into_owned();
        } else {
            let replacement = format!("[REDACTED:{}]", p.kind);
            out = p.re.replace_all(&out, replacement.as_str()).into_owned();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_aws_access_key() {
        let input = "aws key is AKIAIOSFODNN7EXAMPLE in the config";
        let out = mask(input);
        assert!(out.contains("[REDACTED:aws_access_key]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn masks_github_pat() {
        let input = "token=ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let out = mask(input);
        assert!(!out.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        // The generic credential rule runs after the github_pat rule —
        // either label is acceptable so long as the secret is gone.
        assert!(out.contains("REDACTED"));
    }

    #[test]
    fn masks_openai_style_key() {
        let input = "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz1234567890";
        let out = mask(input);
        assert!(!out.contains("sk-proj-abcdefghijklmnopqrstuvwxyz1234567890"));
        assert!(out.contains("REDACTED"));
    }

    #[test]
    fn masks_pem_private_key() {
        let input = "config:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\ngarbage\n-----END RSA PRIVATE KEY-----\nend";
        let out = mask(input);
        assert!(out.contains("[REDACTED:private_key]"));
        assert!(!out.contains("MIIEpAIBAAKCAQEA"));
        assert!(out.starts_with("config:"));
        assert!(out.trim_end().ends_with("end"));
    }

    #[test]
    fn masks_generic_credential_assignment() {
        let cases = [
            "api_key = \"abcd1234efgh5678\"",
            "API-KEY: s3cret_v@lue_12345",
            "password=hunter2hunter2",
            "auth_token = longlonglonglonglonglonglongvalue",
        ];
        for c in cases {
            let out = mask(c);
            assert!(
                out.contains("[REDACTED:credential]"),
                "failed to mask: {c} -> {out}",
            );
        }
    }

    #[test]
    fn preserves_non_secret_text() {
        let input = "the quick brown fox jumps over the lazy dog 42 times";
        let out = mask(input);
        assert_eq!(out, input);
    }

    #[test]
    fn does_not_mask_short_values() {
        // 4-char password should not trip the generic credential rule
        // (requires 8+ chars to avoid masking common identifiers).
        let input = "id = abcd";
        let out = mask(input);
        assert_eq!(out, input);
    }

    #[test]
    fn idempotent() {
        let input = "key=sk-abcdefghijklmnopqrstuvwxyz and AKIAIOSFODNN7EXAMPLE";
        let once = mask(input);
        let twice = mask(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn masks_multiple_secrets_in_one_string() {
        let input = "aws=AKIAIOSFODNN7EXAMPLE gh=ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let out = mask(input);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!out.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
    }

    #[test]
    fn allows_legitimate_code_through() {
        let code = r#"
            fn add(a: i32, b: i32) -> i32 { a + b }
            let x = vec![1, 2, 3];
            println!("{}", x.len());
        "#;
        let out = mask(code);
        assert_eq!(out, code);
    }
}
