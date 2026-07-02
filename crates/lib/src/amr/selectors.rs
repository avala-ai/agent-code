//! Selectors: deterministic relevance tests that run over the whole tree
//! with no model in the loop.
//!
//! Two kinds cover the security profile's needs:
//!
//! - [`SelectorKind::Lexical`] — a regex over raw file text. Cheap, works
//!   on any language, catches repo-specific conventions and string sinks.
//! - [`SelectorKind::Ast`] — matches syntax nodes (e.g. call expressions
//!   whose callee is a dangerous API). Precise: a `call` node is a real
//!   call, not the word `eval` inside a comment or string. Requires a
//!   tree-sitter grammar for the file's language; when none is available
//!   the selector simply emits nothing and lexical selectors still apply.
//!
//! Every match becomes a [`Signal`]. Files that produce no signals never
//! reach the expensive MAP stage.

use std::path::Path;

use regex::Regex;
use tree_sitter::{Language, Parser};

use super::types::Signal;

/// Source language, derived from a file extension. Only a subset has a
/// bundled grammar; the rest are still usable by lexical selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    JavaScript,
    TypeScript,
    Go,
    Rust,
    Ruby,
    Java,
    C,
    Cpp,
    Php,
}

impl Lang {
    /// Infer the language from a path's extension, or `None` for files we
    /// do not classify (which lexical selectors may still scan as "any").
    pub fn from_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "py" | "pyi" => Lang::Python,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "ts" | "tsx" => Lang::TypeScript,
            "go" => Lang::Go,
            "rs" => Lang::Rust,
            "rb" => Lang::Ruby,
            "java" => Lang::Java,
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Lang::Cpp,
            "php" | "php5" | "phtml" => Lang::Php,
            _ => return None,
        })
    }

    /// The tree-sitter grammar for this language, if one is bundled.
    ///
    /// Only Python and JavaScript ship in the vertical slice; other
    /// languages fall back to lexical selection. TypeScript intentionally
    /// returns `None` rather than borrowing the JS grammar, which would
    /// misparse type syntax.
    pub fn tree_sitter_language(self) -> Option<Language> {
        match self {
            Lang::Python => Some(Language::from(tree_sitter_python::LANGUAGE)),
            Lang::JavaScript => Some(Language::from(tree_sitter_javascript::LANGUAGE)),
            _ => None,
        }
    }
}

/// How a selector decides a file (or node) is relevant.
#[derive(Debug, Clone)]
pub enum SelectorKind {
    /// Regex over the raw file text.
    Lexical { pattern: Regex },
    /// Match syntax nodes whose `kind()` is in `node_kinds` and whose
    /// source text matches `callee` (when set). For dangerous-call
    /// selection, `node_kinds` is the language's call-expression kinds and
    /// `callee` is the API name.
    Ast {
        node_kinds: Vec<String>,
        callee: Option<Regex>,
    },
}

/// A named relevance test. Persisted (in the profile) and reusable.
#[derive(Debug, Clone)]
pub struct Selector {
    pub id: String,
    pub description: String,
    /// Languages this selector applies to. Empty means "any file".
    pub langs: Vec<Lang>,
    pub kind: SelectorKind,
}

/// Cap on signals a single selector may emit for a single file, so a
/// pathological file cannot flood the batcher.
const MAX_SIGNALS_PER_SELECTOR_FILE: usize = 100;

impl Selector {
    fn applies_to(&self, lang: Option<Lang>) -> bool {
        if self.langs.is_empty() {
            return true;
        }
        match lang {
            Some(l) => self.langs.contains(&l),
            None => false,
        }
    }

    /// Emit signals for this selector over `text` of the file at
    /// `rel_path` (a repo-relative path). Signals are returned sorted by
    /// byte offset so output is deterministic regardless of traversal.
    pub fn scan_text(&self, rel_path: &Path, text: &str) -> Vec<Signal> {
        let lang = Lang::from_path(rel_path);
        if !self.applies_to(lang) {
            return vec![];
        }
        let mut signals = match &self.kind {
            SelectorKind::Lexical { pattern } => self.lexical(rel_path, text, pattern),
            SelectorKind::Ast { node_kinds, callee } => match lang
                .and_then(|l| l.tree_sitter_language())
            {
                Some(language) => self.ast(rel_path, text, &language, node_kinds, callee.as_ref()),
                None => vec![],
            },
        };
        signals.sort_by_key(|s| s.byte_range.map(|r| r.0).unwrap_or(0));
        signals
    }

    fn lexical(&self, rel_path: &Path, text: &str, pattern: &Regex) -> Vec<Signal> {
        let mut out = Vec::new();
        for m in pattern.find_iter(text) {
            if out.len() >= MAX_SIGNALS_PER_SELECTOR_FILE {
                break;
            }
            out.push(Signal {
                file: rel_path.to_path_buf(),
                line: Some(line_of(text, m.start())),
                byte_range: Some((m.start(), m.end())),
                selector_id: self.id.clone(),
                evidence: line_snippet(text, m.start()),
            });
        }
        out
    }

    fn ast(
        &self,
        rel_path: &Path,
        text: &str,
        language: &Language,
        node_kinds: &[String],
        callee: Option<&Regex>,
    ) -> Vec<Signal> {
        let mut parser = Parser::new();
        if parser.set_language(language).is_err() {
            return vec![];
        }
        let Some(tree) = parser.parse(text, None) else {
            return vec![];
        };
        let mut out = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if out.len() >= MAX_SIGNALS_PER_SELECTOR_FILE {
                break;
            }
            if node_kinds.iter().any(|k| k == node.kind()) {
                let end = node.end_byte().min(text.len());
                let node_text = &text[node.start_byte()..end];
                let hit = callee.map(|re| re.is_match(node_text)).unwrap_or(true);
                if hit {
                    out.push(Signal {
                        file: rel_path.to_path_buf(),
                        line: Some(node.start_position().row + 1),
                        byte_range: Some((node.start_byte(), end)),
                        selector_id: self.id.clone(),
                        evidence: truncate(node_text.trim(), 200),
                    });
                }
            }
            let count = node.child_count();
            for i in 0..count {
                if let Some(child) = node.child(i as u32) {
                    stack.push(child);
                }
            }
        }
        out
    }
}

/// 1-based line number of the byte offset `byte` within `text`.
fn line_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

/// The trimmed source line containing `byte`, bounded in length.
fn line_snippet(text: &str, byte: usize) -> String {
    let byte = byte.min(text.len());
    let start = text[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[byte..]
        .find('\n')
        .map(|i| byte + i)
        .unwrap_or(text.len());
    truncate(text[start..end].trim(), 200)
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lexical(id: &str, langs: Vec<Lang>, re: &str) -> Selector {
        Selector {
            id: id.into(),
            description: String::new(),
            langs,
            kind: SelectorKind::Lexical {
                pattern: Regex::new(re).unwrap(),
            },
        }
    }

    #[test]
    fn lang_from_extension() {
        assert_eq!(Lang::from_path(Path::new("a/b.py")), Some(Lang::Python));
        assert_eq!(Lang::from_path(Path::new("x.jsx")), Some(Lang::JavaScript));
        assert_eq!(Lang::from_path(Path::new("x.ts")), Some(Lang::TypeScript));
        assert_eq!(Lang::from_path(Path::new("README")), None);
    }

    #[test]
    fn python_and_javascript_have_grammars() {
        assert!(Lang::Python.tree_sitter_language().is_some());
        assert!(Lang::JavaScript.tree_sitter_language().is_some());
        // TypeScript deliberately has no grammar in the slice.
        assert!(Lang::TypeScript.tree_sitter_language().is_none());
        assert!(Lang::Rust.tree_sitter_language().is_none());
    }

    #[test]
    fn lexical_selector_reports_line_and_evidence() {
        let sel = lexical("dangerous.eval", vec![Lang::Python], r"eval\s*\(");
        let text = "import os\nx = 1\nresult = eval(user_input)\n";
        let signals = sel.scan_text(Path::new("app.py"), text);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].line, Some(3));
        assert_eq!(signals[0].selector_id, "dangerous.eval");
        assert!(signals[0].evidence.contains("eval(user_input)"));
    }

    #[test]
    fn lexical_selector_respects_language_scope() {
        let sel = lexical("dangerous.eval", vec![Lang::Python], r"eval\s*\(");
        // A JS file must not match a Python-scoped selector.
        let signals = sel.scan_text(Path::new("app.js"), "eval(x)\n");
        assert!(signals.is_empty());
    }

    #[test]
    fn any_language_selector_matches_unknown_extension() {
        let sel = lexical("secret.aws", vec![], r"AKIA[0-9A-Z]{16}");
        let signals = sel.scan_text(Path::new("notes.txt"), "key=AKIA0123456789ABCDEF\n");
        assert_eq!(signals.len(), 1);
    }

    #[test]
    fn ast_selector_matches_python_call_node() {
        let sel = Selector {
            id: "dangerous.os_system".into(),
            description: String::new(),
            langs: vec![Lang::Python],
            kind: SelectorKind::Ast {
                node_kinds: vec!["call".into()],
                callee: Some(Regex::new(r"os\.system").unwrap()),
            },
        };
        let text =
            "import os\n# os.system in a comment should not count as a call\nos.system(cmd)\n";
        let signals = sel.scan_text(Path::new("run.py"), text);
        assert_eq!(signals.len(), 1, "only the real call node should match");
        assert_eq!(signals[0].line, Some(3));
    }

    #[test]
    fn ast_selector_on_language_without_grammar_is_empty() {
        let sel = Selector {
            id: "dangerous.call".into(),
            description: String::new(),
            langs: vec![Lang::Rust],
            kind: SelectorKind::Ast {
                node_kinds: vec!["call_expression".into()],
                callee: None,
            },
        };
        // Rust has no bundled grammar → graceful empty, no panic.
        assert!(
            sel.scan_text(Path::new("main.rs"), "std::process::Command::new(x)\n")
                .is_empty()
        );
    }

    #[test]
    fn signals_are_sorted_by_offset() {
        let sel = lexical("kw.todo", vec![], r"TODO");
        let text = "line1 TODO\nline2\nline3 TODO\n";
        let signals = sel.scan_text(&PathBuf::from("f.txt"), text);
        assert_eq!(signals.len(), 2);
        assert!(signals[0].byte_range.unwrap().0 < signals[1].byte_range.unwrap().0);
    }
}
