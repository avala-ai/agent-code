//! Terminal rendering for markdown and code.
//!
//! Converts markdown text to styled terminal output with syntax
//! highlighting for fenced code blocks. Uses termimad for markdown
//! structure and syntect for code coloring.

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

/// Render markdown text to the terminal with syntax highlighting.
///
/// Handles fenced code blocks (```lang ... ```) with syntect coloring,
/// and delegates the rest to termimad for structural rendering.
pub fn render_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End of code block — render with syntax highlighting.
                output.push_str(&highlight_code(&code_buffer, &code_lang));
                output.push('\n');
                code_buffer.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                // Start of code block — extract language hint.
                code_lang = line.trim_start_matches('`').trim().to_string();
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            code_buffer.push_str(line);
            code_buffer.push('\n');
        } else {
            // Render inline markdown elements.
            output.push_str(&render_inline(line));
            output.push('\n');
        }
    }

    // Handle unclosed code block.
    if in_code_block && !code_buffer.is_empty() {
        output.push_str(&highlight_code(&code_buffer, &code_lang));
        output.push('\n');
    }

    output
}

/// Syntax-highlight a code block using syntect.
fn highlight_code(code: &str, lang: &str) -> String {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    // Find syntax by language name or extension.
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = String::new();

    for line in LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, &ss) {
            Ok(ranges) => {
                let escaped = as_24_bit_terminal_escaped(&ranges, false);
                result.push_str(&escaped);
            }
            Err(_) => {
                result.push_str(line);
            }
        }
    }

    // Reset terminal colors.
    result.push_str("\x1b[0m");
    result
}

/// Render inline markdown elements (bold, italic, code spans, links).
fn render_inline(line: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Inline code: `code`
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                let code: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("\x1b[36m{code}\x1b[0m")); // cyan
                i = end + 1;
                continue;
            }
        }

        // Bold: **text** or __text__
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_double_closing(&chars, i + 2, '*') {
                let text: String = chars[i + 2..end].iter().collect();
                result.push_str(&format!("\x1b[1m{text}\x1b[0m")); // bold
                i = end + 2;
                continue;
            }
        }

        // Italic: *text* or _text_
        if chars[i] == '*' || chars[i] == '_' {
            let marker = chars[i];
            if i + 1 < chars.len() && chars[i + 1] != ' ' {
                if let Some(end) = find_closing(&chars, i + 1, marker) {
                    if end > i + 1 {
                        let text: String = chars[i + 1..end].iter().collect();
                        result.push_str(&format!("\x1b[3m{text}\x1b[0m")); // italic
                        i = end + 1;
                        continue;
                    }
                }
            }
        }

        // Headings: # at start of line.
        if i == 0 && chars[i] == '#' {
            let level = chars.iter().take_while(|&&c| c == '#').count();
            let text: String = chars[level..].iter().collect();
            let text = text.trim_start();
            result.push_str(&format!("\x1b[1;4m{text}\x1b[0m")); // bold + underline
            return result;
        }

        // List items: - or * at start.
        if i == 0 && (chars[i] == '-' || chars[i] == '*') && chars.get(1) == Some(&' ') {
            let text: String = chars[2..].iter().collect();
            result.push_str(&format!("  \x1b[36m•\x1b[0m {text}"));
            return result;
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len() {
        if chars[i] == marker {
            return Some(i);
        }
    }
    None
}

fn find_double_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len().saturating_sub(1) {
        if chars[i] == marker && chars[i + 1] == marker {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_heading() {
        let result = render_inline("# Hello World");
        assert!(result.contains("Hello World"));
        assert!(result.contains("\x1b[1;4m")); // bold + underline
    }

    #[test]
    fn test_render_list_item() {
        let result = render_inline("- item one");
        assert!(result.contains("•"));
        assert!(result.contains("item one"));
    }

    #[test]
    fn test_highlight_code_doesnt_panic() {
        let result = highlight_code("fn main() {}\n", "rust");
        assert!(!result.is_empty());
    }
}
