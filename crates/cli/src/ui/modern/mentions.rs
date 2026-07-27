//! `@path` file mentions for the modern TUI.
//!
//! Two halves, both pure functions over `(cwd, text)` so they are testable
//! without a terminal:
//!
//! * [`at_token_at_cursor`] + [`complete_at_path`] — Tab completion of file
//!   and directory paths for the `@`-token under the cursor.
//! * [`expand_mentions`] — inlining the referenced file contents into the
//!   message that goes to the engine on submit.
//!
//! Parsing of `@path` refs is *not* duplicated here: it reuses
//! [`crate::commands::extract_at_mentions`], the same parser `/files` uses.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Per-file cap on inlined content. 64 KiB is roughly 16k tokens — a large
/// source file still lands whole, and a stray `@some.log` cannot dominate the
/// window on its own.
pub const MAX_FILE_BYTES: usize = 64 * 1024;

/// Cap across every mention in a single prompt. 256 KiB (~64k tokens) bounds
/// the worst case at four full-size files; the rest are skipped with a note.
pub const MAX_TOTAL_BYTES: usize = 256 * 1024;

/// Upper bound on directory entries examined for one completion, so a
/// pathological directory cannot stall the UI thread.
pub const MAX_SCAN_ENTRIES: usize = 4_000;

/// Entries listed when a mention resolves to a directory.
const MAX_DIR_ENTRIES: usize = 100;

/// Bytes inspected when sniffing for binary content.
const BINARY_SNIFF_BYTES: usize = 8_192;

/// Trailing sentence punctuation stripped when the literal mention does not
/// exist on disk (`look at @src/main.rs.`).
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')'];

/// The `@…` token under the cursor: byte range in the input plus the text
/// after the `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtToken {
    pub start: usize,
    pub end: usize,
    pub partial: String,
}

/// Locate the whitespace-delimited `@` token the cursor sits in.
///
/// The token must start at the beginning of the input or after whitespace —
/// the same rule [`crate::commands::extract_at_mentions`] applies, so an
/// email address (`a@b.com`) never looks like a mention. The cursor must be
/// past the `@` itself; sitting immediately before it is not "inside".
pub fn at_token_at_cursor(input: &str, cursor: usize) -> Option<AtToken> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) {
        return None;
    }
    let start = input[..cursor]
        .rfind(char::is_whitespace)
        .map(|i| i + input[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    if start >= cursor || !input[start..].starts_with('@') {
        return None;
    }
    let end = input[start..]
        .find(char::is_whitespace)
        .map(|i| start + i)
        .unwrap_or(input.len());
    Some(AtToken {
        start,
        end,
        partial: input[start + 1..end].to_string(),
    })
}

/// Longest common prefix of `items`, on char boundaries.
pub fn longest_common_prefix<S: AsRef<str>>(items: &[S]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix = first.as_ref().to_string();
    for item in &items[1..] {
        while !item.as_ref().starts_with(&prefix) && !prefix.is_empty() {
            prefix.pop();
        }
    }
    prefix
}

/// Complete `partial` (the text after `@`) into workspace-relative paths.
///
/// Directories come back with a trailing `/` so the user can keep drilling
/// in. `.gitignore` is honoured — `require_git(false)` so it also applies in a
/// directory that is not a git checkout — and `.git/` is never offered.
/// Dotfiles are hidden unless the leaf being typed starts with `.`, matching
/// shell completion. Paths that escape `cwd` produce no candidates.
pub fn complete_at_path(cwd: &Path, partial: &str) -> Vec<String> {
    let (dir_prefix, leaf) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };
    let root = cwd.join(dir_prefix);
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    if !root.is_dir() || !contained_in(cwd, &root) {
        return Vec::new();
    }

    let leaf_lower = leaf.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let walker = WalkBuilder::new(&root)
        .max_depth(Some(1))
        .hidden(!leaf.starts_with('.'))
        .follow_links(false)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .parents(true)
        .build();
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        scanned += 1;
        if scanned > MAX_SCAN_ENTRIES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        if !name.to_lowercase().starts_with(&leaf_lower) {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let suffix = if is_dir { "/" } else { "" };
        out.push(format!("{dir_prefix}{name}{suffix}"));
    }
    out.sort();
    out.dedup();
    out
}

/// A completed candidate rewritten so [`crate::commands::extract_at_mentions`]
/// will still recognise it: that parser requires a `/` or `.` in the token, so
/// an extensionless top-level file (`Makefile`) is emitted as `./Makefile`.
pub fn mention_text(candidate: &str) -> String {
    if candidate.contains('/') || candidate.contains('.') {
        candidate.to_string()
    } else {
        format!("./{candidate}")
    }
}

/// Result of inlining `@path` mentions into a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionExpansion {
    /// What the engine should receive: the user's text plus file blocks.
    pub prompt: String,
    /// Short human-readable notes about anything skipped or truncated.
    pub notes: Vec<String>,
    /// Image files to attach to the turn as content blocks. An image
    /// cannot be inlined as text, so `@shot.png` used to report
    /// "binary, skipped" — which is never what the user meant by
    /// mentioning one.
    pub images: Vec<PathBuf>,
}

/// Inline the contents of every `@path` mention in `text`.
///
/// Returns `None` when `text` holds no mentions, so the submit path is
/// untouched for ordinary prompts. The user's own text is always the prefix
/// of `prompt` — expansion only appends.
pub fn expand_mentions(text: &str, cwd: &Path) -> Option<MentionExpansion> {
    let mentions = crate::commands::extract_at_mentions(text);
    if mentions.is_empty() {
        return None;
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut body = String::new();
    let mut notes: Vec<String> = Vec::new();
    let mut images: Vec<PathBuf> = Vec::new();
    let mut used = 0usize;
    let mut over_budget = 0usize;

    for raw in mentions {
        if !seen.insert(raw.clone()) {
            continue;
        }
        let resolved = match resolve_mention(cwd, &raw) {
            Ok(r) => r,
            Err(reason) => {
                notes.push(format!("@{raw} — {reason}"));
                continue;
            }
        };
        let remaining = MAX_TOTAL_BYTES.saturating_sub(used);
        if remaining == 0 {
            over_budget += 1;
            continue;
        }
        let cap = MAX_FILE_BYTES.min(remaining);
        let (label, kind, content) = match resolved {
            Resolved::Dir(path) => {
                let label = display_path(cwd, &path, &raw);
                (label, "directory", list_dir(&path))
            }
            Resolved::File(path) if is_image(&path) => {
                // Attached rather than inlined; the model receives it as
                // an image block on the turn.
                let label = display_path(cwd, &path, &raw);
                notes.push(format!("@{raw} — attached as an image"));
                images.push(path);
                let _ = label;
                continue;
            }
            Resolved::File(path) => match read_text_capped(&path, cap) {
                Ok(content) => (display_path(cwd, &path, &raw), "file", content),
                Err(reason) => {
                    notes.push(format!("@{raw} — {reason}"));
                    continue;
                }
            },
        };
        let (content, cut) = truncate_utf8(content, cap);
        if let Some(shown) = cut {
            notes.push(format!("@{raw} — truncated to {} KiB", shown / 1024));
        }
        used += content.len();
        if body.is_empty() {
            body.push_str("\n\nReferenced files (expanded from @ mentions above):\n");
        }
        body.push_str(&format!(
            "\n<{kind} path=\"{label}\">\n{content}\n</{kind}>\n"
        ));
    }

    if over_budget > 0 {
        notes.push(format!(
            "{over_budget} mention(s) skipped — {} KiB total limit reached",
            MAX_TOTAL_BYTES / 1024
        ));
    }

    Some(MentionExpansion {
        prompt: format!("{text}{body}"),
        notes,
        images,
    })
}

enum Resolved {
    File(PathBuf),
    Dir(PathBuf),
}

/// Resolve one raw mention against `cwd`, refusing anything that leaves the
/// workspace. Containment is checked on the *canonical* path so a symlink
/// pointing outside the project cannot satisfy a lexical prefix test.
fn resolve_mention(cwd: &Path, raw: &str) -> Result<Resolved, String> {
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("not a usable path".into());
    }

    let stripped = raw.trim_end_matches(TRAILING_PUNCTUATION);
    let mut canon = None;
    for candidate in [raw, stripped] {
        if candidate.is_empty() {
            continue;
        }
        let p = Path::new(candidate);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        };
        if let Ok(c) = abs.canonicalize() {
            canon = Some(c);
            break;
        }
    }
    let Some(canon) = canon else {
        return Err("not found".into());
    };

    if !contained_in(cwd, &canon) {
        return Err("outside the workspace".into());
    }
    let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if let Ok(rel) = canon.strip_prefix(&cwd_canon)
        && rel.components().any(|c| c.as_os_str() == ".git")
    {
        return Err("inside .git/".into());
    }

    if canon.is_dir() {
        Ok(Resolved::Dir(canon))
    } else if canon.is_file() {
        Ok(Resolved::File(canon))
    } else {
        Err("not a regular file".into())
    }
}

/// Extensions the model can be shown directly. Kept to the formats the
/// API accepts, so an unsupported image still reports a clear reason
/// rather than being attached and rejected upstream.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// True when `path` resolves inside `cwd`. Both sides are canonicalized.
fn contained_in(cwd: &Path, path: &Path) -> bool {
    let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let path_canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path_canon.starts_with(&cwd_canon)
}

/// Label a resolved path for the model: workspace-relative when possible.
///
/// Joined with forward slashes on every platform: the tag is
/// model-facing text, so the same mention must serialize identically
/// regardless of host OS — `to_string_lossy` on the relative path
/// produced `src\main.rs` on Windows.
fn display_path(cwd: &Path, path: &Path, raw: &str) -> String {
    let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    path.strip_prefix(&cwd_canon)
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
                .replace('"', "'")
        })
        .unwrap_or_else(|_| raw.replace('"', "'"))
}

/// Read at most `cap` bytes of `path` as UTF-8 text.
///
/// Reads `cap + 1` bytes at most so a huge file never lands in memory, then
/// rejects anything that sniffs binary or fails to decode.
fn read_text_capped(path: &Path, cap: usize) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("unreadable ({})", e.kind()))?;
    let mut data = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("unreadable ({})", e.kind()))?;

    let sniff_len = data.len().min(BINARY_SNIFF_BYTES);
    if data[..sniff_len].contains(&0) {
        return Err("binary, skipped".into());
    }
    // A read cut at `cap` can land mid-character; that trailing fragment is
    // dropped rather than treated as "not text".
    match std::str::from_utf8(&data) {
        Ok(s) => Ok(s.to_string()),
        Err(e) if e.error_len().is_none() && data.len() > cap => {
            Ok(String::from_utf8_lossy(&data[..e.valid_up_to()]).into_owned())
        }
        Err(_) => Err("not valid UTF-8, skipped".into()),
    }
}

/// Render a directory mention as a one-level listing rather than skipping it:
/// cheap, bounded, and usually what the user meant by `@src/`.
fn list_dir(dir: &Path) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(true)
        .follow_links(false)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .parents(true)
        .build();
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        scanned += 1;
        if scanned > MAX_SCAN_ENTRIES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let suffix = if entry.file_type().is_some_and(|t| t.is_dir()) {
            "/"
        } else {
            ""
        };
        entries.push(format!("{name}{suffix}"));
    }
    entries.sort();
    let total = entries.len();
    entries.truncate(MAX_DIR_ENTRIES);
    if total > MAX_DIR_ENTRIES {
        entries.push(format!("… +{} more", total - MAX_DIR_ENTRIES));
    }
    if entries.is_empty() {
        "(no entries)".to_string()
    } else {
        entries.join("\n")
    }
}

/// Cut `s` to at most `cap` bytes on a char boundary, appending a marker.
/// Returns the bytes actually shown when a cut happened.
fn truncate_utf8(s: String, cap: usize) -> (String, Option<usize>) {
    if s.len() <= cap {
        return (s, None);
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str(&format!(
        "\n… [truncated: first {end} bytes of at least {} shown]",
        s.len()
    ));
    (out, Some(end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("src/inner")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        fs::write(root.join("src/inner/deep.rs"), "// deep\n").unwrap();
        fs::write(root.join("README.md"), "# readme\n").unwrap();
        fs::write(root.join("Makefile"), "all:\n").unwrap();
        fs::write(root.join(".gitignore"), "secret.txt\ntarget/\n").unwrap();
        fs::write(root.join("secret.txt"), "hunter2\n").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/build.log"), "noise\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[core]\n").unwrap();
        dir
    }

    // ---- token under cursor ----

    #[test]
    fn token_at_cursor_finds_single_mention() {
        let tok = at_token_at_cursor("look at @src/ma", 15).expect("token");
        assert_eq!(tok.partial, "src/ma");
        assert_eq!(&"look at @src/ma"[tok.start..tok.end], "@src/ma");
    }

    #[test]
    fn token_at_cursor_picks_the_mention_under_the_cursor() {
        let input = "@src/main.rs and @READ more";
        // Cursor at the end of the second mention.
        let tok = at_token_at_cursor(input, 21).expect("second token");
        assert_eq!(tok.partial, "READ");
        // Cursor inside the first mention.
        let tok = at_token_at_cursor(input, 4).expect("first token");
        assert_eq!(tok.partial, "src/main.rs");
        assert_eq!(tok.start, 0);
    }

    #[test]
    fn token_at_cursor_ignores_email_and_plain_text() {
        assert!(at_token_at_cursor("mail me at a@b.com", 18).is_none());
        assert!(at_token_at_cursor("just words", 5).is_none());
        // Cursor sitting before the `@` is not inside the token.
        assert!(at_token_at_cursor("@src", 0).is_none());
    }

    #[test]
    fn token_at_cursor_handles_multibyte_input() {
        let input = "héllo @sr";
        let tok = at_token_at_cursor(input, input.len()).expect("token");
        assert_eq!(tok.partial, "sr");
    }

    // ---- completion ----

    #[test]
    fn completes_directory_contents_with_trailing_slash() {
        let dir = fixture();
        let cands = complete_at_path(dir.path(), "src/");
        assert!(cands.contains(&"src/main.rs".to_string()));
        assert!(cands.contains(&"src/lib.rs".to_string()));
        assert!(
            cands.contains(&"src/inner/".to_string()),
            "directories get a trailing slash: {cands:?}"
        );
    }

    #[test]
    fn completion_filters_by_leaf_prefix_case_insensitively() {
        let dir = fixture();
        assert_eq!(complete_at_path(dir.path(), "src/ma"), vec!["src/main.rs"]);
        assert_eq!(complete_at_path(dir.path(), "readme"), vec!["README.md"]);
    }

    #[test]
    fn completion_excludes_gitignored_and_git_dir() {
        let dir = fixture();
        let cands = complete_at_path(dir.path(), "");
        assert!(
            !cands.iter().any(|c| c.starts_with("secret.txt")),
            "gitignored file offered: {cands:?}"
        );
        assert!(
            !cands.iter().any(|c| c.starts_with("target")),
            "gitignored dir offered: {cands:?}"
        );
        assert!(
            !cands.iter().any(|c| c.starts_with(".git")),
            ".git offered: {cands:?}"
        );
        assert!(cands.contains(&"README.md".to_string()));
    }

    #[test]
    fn completion_refuses_to_escape_the_workspace() {
        let dir = fixture();
        let nested = dir.path().join("src");
        assert!(complete_at_path(&nested, "../").is_empty());
        assert!(complete_at_path(dir.path(), "/etc/").is_empty());
    }

    #[test]
    fn completion_of_unknown_prefix_is_empty() {
        let dir = fixture();
        assert!(complete_at_path(dir.path(), "nope/").is_empty());
        assert!(complete_at_path(dir.path(), "zzz").is_empty());
    }

    #[test]
    fn mention_text_keeps_extensionless_names_parseable() {
        assert_eq!(mention_text("Makefile"), "./Makefile");
        assert_eq!(mention_text("src/"), "src/");
        assert_eq!(mention_text("README.md"), "README.md");
        // The rewritten form survives the shared parser.
        assert_eq!(
            crate::commands::extract_at_mentions("see @./Makefile"),
            vec!["./Makefile".to_string()]
        );
    }

    #[test]
    fn longest_common_prefix_stops_at_divergence() {
        assert_eq!(longest_common_prefix(&["abc", "abd"]), "ab");
        assert_eq!(longest_common_prefix(&["abc"]), "abc");
        assert_eq!(longest_common_prefix(&["abc", "xyz"]), "");
        assert_eq!(longest_common_prefix::<&str>(&[]), "");
    }

    // ---- expansion ----

    #[test]
    fn expand_returns_none_without_mentions() {
        let dir = fixture();
        assert!(expand_mentions("just a prompt", dir.path()).is_none());
    }

    #[test]
    fn expand_inlines_a_single_file() {
        let dir = fixture();
        let out = expand_mentions("explain @src/main.rs please", dir.path()).expect("expanded");
        assert!(out.prompt.starts_with("explain @src/main.rs please"));
        assert!(out.prompt.contains("<file path=\"src/main.rs\">"));
        assert!(out.prompt.contains("fn main() {}"));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn expand_inlines_multiple_files_once_each() {
        let dir = fixture();
        let out = expand_mentions(
            "compare @src/main.rs @src/lib.rs and again @src/main.rs",
            dir.path(),
        )
        .expect("expanded");
        assert_eq!(out.prompt.matches("<file path=\"src/main.rs\">").count(), 1);
        assert_eq!(out.prompt.matches("<file path=\"src/lib.rs\">").count(), 1);
        assert!(out.prompt.contains("pub fn lib() {}"));
    }

    #[test]
    fn expand_notes_missing_file_and_keeps_the_turn() {
        let dir = fixture();
        let out = expand_mentions("read @nope/missing.rs", dir.path()).expect("expanded");
        assert_eq!(out.prompt, "read @nope/missing.rs");
        assert_eq!(out.notes, vec!["@nope/missing.rs — not found".to_string()]);
    }

    #[test]
    fn expand_rejects_paths_escaping_the_workspace() {
        let dir = fixture();
        let nested = dir.path().join("src");
        let out = expand_mentions("see @../README.md", &nested).expect("expanded");
        assert_eq!(out.notes, vec!["@../README.md — outside the workspace"]);
        assert!(!out.prompt.contains("<file"));

        let out = expand_mentions("see @/etc/hostname", dir.path()).expect("expanded");
        assert_eq!(out.notes.len(), 1);
        assert!(
            out.notes[0].contains("outside the workspace") || out.notes[0].contains("not found"),
            "{:?}",
            out.notes
        );
        assert!(!out.prompt.contains("<file"));
    }

    // Unix-only: creating a symlink on Windows needs developer mode or
    // SeCreateSymbolicLinkPrivilege, neither of which CI runners grant.
    // Gating the whole test keeps `dir` used on every target.
    #[cfg(unix)]
    #[test]
    fn expand_rejects_symlink_escaping_the_workspace() {
        let dir = fixture();
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("secret.env"), "TOKEN=1\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.env"),
            dir.path().join("link.env"),
        )
        .unwrap();
        let out = expand_mentions("see @link.env", dir.path()).expect("expanded");
        assert_eq!(out.notes, vec!["@link.env — outside the workspace"]);
        assert!(!out.prompt.contains("TOKEN=1"));
    }

    #[test]
    fn expand_skips_the_git_directory() {
        let dir = fixture();
        let out = expand_mentions("see @.git/config", dir.path()).expect("expanded");
        assert_eq!(out.notes, vec!["@.git/config — inside .git/"]);
        assert!(!out.prompt.contains("[core]"));
    }

    #[test]
    fn expand_truncates_an_oversized_file_with_a_marker() {
        let dir = fixture();
        let big = "x".repeat(MAX_FILE_BYTES + 4_096);
        fs::write(dir.path().join("big.txt"), &big).unwrap();
        let out = expand_mentions("read @big.txt", dir.path()).expect("expanded");
        assert!(out.prompt.contains("… [truncated:"), "no marker");
        assert!(out.prompt.len() < MAX_FILE_BYTES + 2_048);
        assert_eq!(out.notes.len(), 1);
        assert!(out.notes[0].contains("truncated"), "{:?}", out.notes);
    }

    #[test]
    fn expand_stops_at_the_total_cap_across_many_mentions() {
        let dir = fixture();
        let chunk = "y".repeat(MAX_FILE_BYTES);
        let mut text = String::from("read these");
        for i in 0..8 {
            fs::write(dir.path().join(format!("f{i}.txt")), &chunk).unwrap();
            text.push_str(&format!(" @f{i}.txt"));
        }
        let out = expand_mentions(&text, dir.path()).expect("expanded");
        assert!(
            out.prompt.len() < MAX_TOTAL_BYTES + 8_192,
            "total cap not enforced: {}",
            out.prompt.len()
        );
        assert!(
            out.notes.iter().any(|n| n.contains("total limit reached")),
            "{:?}",
            out.notes
        );
        // The first four files fit exactly; the rest are skipped.
        assert_eq!(out.prompt.matches("<file path=").count(), 4);
    }

    /// An image cannot be inlined as text, so `@shot.png` used to report
    /// "binary, skipped" — never what mentioning an image means.
    #[test]
    fn an_image_mention_is_attached_not_skipped() {
        let dir = fixture();
        // A real PNG header, so the binary sniff would have caught it.
        fs::write(
            dir.path().join("shot.png"),
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2],
        )
        .unwrap();
        let out = expand_mentions("look at @shot.png", dir.path()).expect("expanded");
        assert_eq!(
            out.images.len(),
            1,
            "image was not attached: {:?}",
            out.notes
        );
        assert!(out.images[0].ends_with("shot.png"));
        assert!(
            out.notes.iter().any(|n| n.contains("attached as an image")),
            "no note explaining the attachment: {:?}",
            out.notes
        );
        assert!(
            !out.prompt.contains("<file"),
            "an image must not be inlined as text"
        );
    }

    #[test]
    fn image_extensions_are_matched_case_insensitively() {
        let dir = fixture();
        for name in ["a.PNG", "b.Jpeg", "c.webp"] {
            fs::write(dir.path().join(name), [0x89, 0, 1]).unwrap();
        }
        let out = expand_mentions("@a.PNG @b.Jpeg @c.webp", dir.path()).expect("expanded");
        assert_eq!(out.images.len(), 3, "{:?}", out.notes);
    }

    /// A non-image binary still reports why it was skipped, rather than
    /// being attached as something the model cannot read.
    #[test]
    fn a_non_image_binary_is_still_skipped() {
        let dir = fixture();
        fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
        let out = expand_mentions("see @blob.bin", dir.path()).expect("expanded");
        assert!(out.images.is_empty(), "attached a non-image binary");
        assert_eq!(out.notes, vec!["@blob.bin — binary, skipped"]);
    }

    #[test]
    fn expand_skips_binary_files() {
        let dir = fixture();
        fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
        let out = expand_mentions("look at @blob.bin", dir.path()).expect("expanded");
        assert_eq!(out.notes, vec!["@blob.bin — binary, skipped"]);
        assert!(!out.prompt.contains("<file"));
    }

    #[test]
    fn expand_lists_a_directory_mention() {
        let dir = fixture();
        let out = expand_mentions("what is in @src/", dir.path()).expect("expanded");
        assert!(out.prompt.contains("<directory path=\"src\">"));
        assert!(out.prompt.contains("main.rs"));
        assert!(out.prompt.contains("inner/"));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn expand_ignores_email_addresses() {
        let dir = fixture();
        assert!(expand_mentions("ping me at a@src/main.rs", dir.path()).is_none());
        assert!(expand_mentions("mail user@example.com", dir.path()).is_none());
    }

    #[test]
    fn expand_tolerates_trailing_sentence_punctuation() {
        let dir = fixture();
        let out = expand_mentions("check @src/main.rs.", dir.path()).expect("expanded");
        assert!(out.prompt.contains("fn main() {}"));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }
}
