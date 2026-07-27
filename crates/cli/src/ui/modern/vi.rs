//! Vi editing mode for the composer.
//!
//! `/vim` set `ui.edit_mode = "vi"` and printed "Takes effect on next
//! session", but the modern TUI never read the setting — it took effect
//! in no session at all. The command dates from the rustyline REPL,
//! which had vi bindings natively; the REPL was removed and the config
//! key outlived it.
//!
//! The motion set is deliberately small. A partial vi that quietly does
//! the wrong thing on an unimplemented key is worse than one whose edges
//! are obvious, so unknown keys in normal mode do nothing rather than
//! falling through to the global chord handler and firing a shortcut the
//! user did not intend.

use super::app::{App, ComposerMode};

/// Result of offering a key to normal mode.
#[derive(Debug, PartialEq, Eq)]
pub enum ViOutcome {
    /// Normal mode handled it; stop processing.
    Consumed,
    /// Not a normal-mode key; the caller may keep going.
    Fallthrough,
}

impl App {
    /// True when the composer is in vi normal mode.
    pub fn in_normal_mode(&self) -> bool {
        self.vi_mode && self.composer_mode == ComposerMode::Normal
    }

    /// Leave insert mode. Vi puts the cursor on the last typed character
    /// rather than after it.
    pub fn vi_enter_normal(&mut self) {
        if !self.vi_mode {
            return;
        }
        self.composer_mode = ComposerMode::Normal;
        let (start, _) = line_bounds(&self.input, self.cursor);
        self.cursor = prev_boundary(&self.input, self.cursor).max(start);
        self.dirty = true;
    }

    /// Drop a half-typed operator. A pending `d` is only meaningful
    /// between the two keys of one command, so every path that sends,
    /// replaces or discards the draft clears it — otherwise it is still
    /// armed for the next draft and eats its first key.
    pub fn reset_vi_operator(&mut self) {
        self.vi_pending_d = false;
    }

    /// Pull the cursor back onto a character. Normal mode has no
    /// position after the last one; parked there, `x` and `D` have
    /// nothing to act on.
    pub fn clamp_cursor_to_normal_mode(&mut self) {
        let (start, end) = line_bounds(&self.input, self.cursor);
        let last = line_last(&self.input, start, end);
        if self.cursor > last {
            self.cursor = last;
            self.dirty = true;
        }
    }

    pub fn vi_enter_insert(&mut self) {
        self.composer_mode = ComposerMode::Insert;
        self.dirty = true;
    }

    /// Handle a character in normal mode.
    pub fn vi_normal_key(&mut self, c: char) -> ViOutcome {
        // A pending `d` waits for its motion; `dd` is the only one.
        if self.vi_pending_d {
            self.vi_pending_d = false;
            if c == 'd' {
                self.delete_current_line();
            }
            return ViOutcome::Consumed;
        }

        let (start, end) = line_bounds(&self.input, self.cursor);
        match c {
            'h' => self.cursor = prev_boundary(&self.input, self.cursor).max(start),
            'l' => {
                self.cursor =
                    next_boundary(&self.input, self.cursor).min(line_last(&self.input, start, end))
            }
            '0' => self.cursor = start,
            '$' => self.cursor = line_last(&self.input, start, end),
            'w' => {
                // With no word left, `word_forward` returns the buffer
                // end, which is not a character the cursor may sit on in
                // normal mode: `x` and `D` would find nothing to delete.
                let target = word_forward(&self.input, self.cursor);
                let (ws, we) = line_bounds(&self.input, target);
                self.cursor = target.min(line_last(&self.input, ws, we));
            }
            'b' => self.cursor = word_back(&self.input, self.cursor),
            'i' => self.vi_enter_insert(),
            'a' => {
                self.cursor = next_boundary(&self.input, self.cursor).min(end);
                self.vi_enter_insert();
            }
            'I' => {
                self.cursor = start;
                self.vi_enter_insert();
            }
            'A' => {
                self.cursor = end;
                self.vi_enter_insert();
            }
            'x' => {
                if self.cursor < end {
                    self.input.remove(self.cursor);
                    // Deleting the last character on the line leaves the
                    // cursor on the new last one, as vi does.
                    let (_, new_end) = line_bounds(&self.input, self.cursor);
                    if self.cursor >= new_end {
                        self.cursor = prev_boundary(&self.input, self.cursor).max(start);
                    }
                }
            }
            'D' => {
                self.input.replace_range(self.cursor..end, "");
                if self.cursor > start {
                    self.cursor = prev_boundary(&self.input, self.cursor).max(start);
                }
            }
            'C' => {
                self.input.replace_range(self.cursor..end, "");
                self.vi_enter_insert();
            }
            'd' => self.vi_pending_d = true,
            // Unknown normal-mode key: swallow it. Falling through would
            // run a global chord the user never meant to trigger.
            _ => {}
        }
        self.dirty = true;
        ViOutcome::Consumed
    }

    /// `dd`: drop the line under the cursor, not the whole draft.
    fn delete_current_line(&mut self) {
        let (start, end) = line_bounds(&self.input, self.cursor);
        if end < self.input.len() {
            // A following line exists: take this line and its newline,
            // leaving the cursor at the start of what moved up.
            self.input.replace_range(start..end + 1, "");
            self.cursor = start;
        } else if start > 0 {
            // Last line of several: take the newline that precedes it so
            // the draft does not keep a trailing blank line.
            self.input.truncate(start - 1);
            self.cursor = line_bounds(&self.input, self.input.len()).0;
        } else {
            self.input.clear();
            self.cursor = 0;
        }
        self.dirty = true;
    }
}

/// Byte range of the line holding `at`, excluding the trailing newline.
/// The composer holds a whole multi-line draft, so every line command
/// scopes itself to this range instead of to the buffer.
fn line_bounds(s: &str, at: usize) -> (usize, usize) {
    let mut at = at.min(s.len());
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    let start = s[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = s[at..].find('\n').map(|i| at + i).unwrap_or(s.len());
    (start, end)
}

/// Offset of the last character on a line, or the line start when empty.
fn line_last(s: &str, start: usize, end: usize) -> usize {
    if end > start {
        prev_boundary(s, end).max(start)
    } else {
        start
    }
}

fn prev_boundary(s: &str, from: usize) -> usize {
    if from == 0 {
        return 0;
    }
    let mut i = from - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, from: usize) -> usize {
    if from >= s.len() {
        return s.len();
    }
    let mut i = from + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Start of the next word, vi's `w`.
fn word_forward(s: &str, from: usize) -> usize {
    let bytes: Vec<(usize, char)> = s.char_indices().collect();
    let mut idx = bytes
        .iter()
        .position(|(i, _)| *i >= from)
        .unwrap_or(bytes.len());
    // Skip the current word, then the whitespace after it.
    while idx < bytes.len() && !bytes[idx].1.is_whitespace() {
        idx += 1;
    }
    while idx < bytes.len() && bytes[idx].1.is_whitespace() {
        idx += 1;
    }
    bytes.get(idx).map(|(i, _)| *i).unwrap_or(s.len())
}

/// Start of the previous word, vi's `b`.
fn word_back(s: &str, from: usize) -> usize {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut idx = chars
        .iter()
        .position(|(i, _)| *i >= from)
        .unwrap_or(chars.len());
    if idx == 0 {
        return 0;
    }
    idx -= 1;
    while idx > 0 && chars[idx].1.is_whitespace() {
        idx -= 1;
    }
    while idx > 0 && !chars[idx - 1].1.is_whitespace() {
        idx -= 1;
    }
    chars.get(idx).map(|(i, _)| *i).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vi_app(text: &str) -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.vi_mode = true;
        app.input = text.to_string();
        app.cursor = text.len();
        app.composer_mode = ComposerMode::Normal;
        app
    }

    #[test]
    fn motions_move_the_cursor() {
        let mut a = vi_app("hello world");
        a.cursor = 0;
        a.vi_normal_key('l');
        assert_eq!(a.cursor, 1);
        a.vi_normal_key('h');
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('$');
        assert_eq!(a.cursor, 10, "$ sits on the last char, not past it");
        a.vi_normal_key('0');
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('w');
        assert_eq!(a.cursor, 6, "w should reach the start of `world`");
        a.vi_normal_key('b');
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn insert_entries_position_the_cursor_correctly() {
        let mut a = vi_app("abc");
        a.cursor = 1;
        a.vi_normal_key('i');
        assert_eq!((a.cursor, a.composer_mode), (1, ComposerMode::Insert));

        let mut a = vi_app("abc");
        a.cursor = 1;
        a.vi_normal_key('a');
        assert_eq!(a.cursor, 2, "`a` appends after the cursor");

        let mut a = vi_app("abc");
        a.vi_normal_key('I');
        assert_eq!(a.cursor, 0);

        let mut a = vi_app("abc");
        a.cursor = 0;
        a.vi_normal_key('A');
        assert_eq!(a.cursor, 3, "`A` appends at end of line");
    }

    #[test]
    fn edits_change_the_line() {
        let mut a = vi_app("abc");
        a.cursor = 0;
        a.vi_normal_key('x');
        assert_eq!(a.input, "bc");

        let mut a = vi_app("hello world");
        a.cursor = 5;
        a.vi_normal_key('D');
        assert_eq!(a.input, "hello");

        let mut a = vi_app("hello world");
        a.cursor = 5;
        a.vi_normal_key('C');
        assert_eq!(a.input, "hello");
        assert_eq!(a.composer_mode, ComposerMode::Insert, "C enters insert");
    }

    /// `d` alone must not delete; only `dd` clears the line.
    #[test]
    fn d_waits_for_its_motion() {
        let mut a = vi_app("hello");
        a.vi_normal_key('d');
        assert_eq!(a.input, "hello", "a lone `d` deleted something");
        a.vi_normal_key('d');
        assert_eq!(a.input, "", "dd should clear the line");

        // `d` followed by an unknown motion cancels rather than deleting.
        let mut a = vi_app("hello");
        a.vi_normal_key('d');
        a.vi_normal_key('z');
        assert_eq!(a.input, "hello");
    }

    /// The composer holds multi-line drafts. Every line command must act
    /// on the line under the cursor; the first cut cleared the whole
    /// `String`, discarding lines the user never touched.
    #[test]
    fn line_commands_stay_inside_the_current_line() {
        // `dd` on a middle line drops only that line.
        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5; // on "two"
        a.vi_normal_key('d');
        a.vi_normal_key('d');
        assert_eq!(a.input, "one\nthree", "dd ate lines outside the cursor's");
        assert_eq!(
            a.cursor, 4,
            "dd leaves the cursor on the line that moved up"
        );

        // `dd` on the last line takes the newline before it.
        let mut a = vi_app("one\ntwo");
        a.cursor = 5;
        a.vi_normal_key('d');
        a.vi_normal_key('d');
        assert_eq!(a.input, "one");
        assert_eq!(a.cursor, 0);

        // `dd` on the only line still clears the draft.
        let mut a = vi_app("solo");
        a.vi_normal_key('d');
        a.vi_normal_key('d');
        assert_eq!(a.input, "");

        // `0` and `$` bound to the current line.
        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5;
        a.vi_normal_key('0');
        assert_eq!(a.cursor, 4, "0 jumped past the start of the line");
        a.vi_normal_key('$');
        assert_eq!(a.cursor, 6, "$ jumped past the end of the line");

        // `D` and `C` truncate the line, not the draft.
        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5;
        a.vi_normal_key('D');
        assert_eq!(a.input, "one\nt\nthree");

        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5;
        a.vi_normal_key('C');
        assert_eq!(a.input, "one\nt\nthree");
        assert_eq!(a.composer_mode, ComposerMode::Insert);

        // `I` and `A` land on this line's edges.
        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5;
        a.vi_normal_key('I');
        assert_eq!(a.cursor, 4);

        let mut a = vi_app("one\ntwo\nthree");
        a.cursor = 5;
        a.vi_normal_key('A');
        assert_eq!(a.cursor, 7, "A appended past the end of the line");
    }

    /// `w` with no word left must stop on the last character, not past
    /// it: normal mode has no position after the final character, so
    /// `x` and `D` would have found nothing to delete there.
    #[test]
    fn w_stops_on_a_real_character_at_the_end() {
        let mut a = vi_app("hello world");
        a.cursor = 6;
        a.vi_normal_key('w');
        assert_eq!(a.cursor, 10, "w ran past the last character");
        a.vi_normal_key('x');
        assert_eq!(a.input, "hello worl", "x had nothing under the cursor");

        // Same on the last line of a multi-line draft.
        let mut a = vi_app("one\ntwo");
        a.cursor = 4;
        a.vi_normal_key('w');
        assert_eq!(a.cursor, 6);
        a.vi_normal_key('D');
        assert_eq!(a.input, "one\ntw");

        // A trailing space must not park the cursor on the boundary
        // either.
        let mut a = vi_app("hi ");
        a.cursor = 0;
        a.vi_normal_key('w');
        assert!(a.cursor < a.input.len(), "w landed past the end");
    }

    /// `h`, `l`, `a` and `x` must not step over a newline either — vi
    /// keeps character motions within the line.
    #[test]
    fn character_motions_do_not_cross_newlines() {
        let mut a = vi_app("ab\ncd");
        a.cursor = 3; // on 'c'
        a.vi_normal_key('h');
        assert_eq!(a.cursor, 3, "h crossed onto the previous line");

        let mut a = vi_app("ab\ncd");
        a.cursor = 1; // on 'b', the last char of line one
        a.vi_normal_key('l');
        assert_eq!(a.cursor, 1, "l crossed onto the next line");

        let mut a = vi_app("ab\ncd");
        a.cursor = 1;
        a.vi_normal_key('a');
        assert_eq!(a.cursor, 2, "a should append at the end of its own line");

        // `x` on an empty line has nothing to delete and must not eat the
        // newline, which would join two lines of the draft.
        let mut a = vi_app("ab\n\ncd");
        a.cursor = 3;
        a.vi_normal_key('x');
        assert_eq!(a.input, "ab\n\ncd", "x swallowed a newline");
    }

    /// An unhandled normal-mode key must be swallowed, not passed on —
    /// otherwise `q` or `t` would fire a global chord mid-edit.
    #[test]
    fn unknown_normal_keys_are_consumed() {
        let mut a = vi_app("hello");
        assert_eq!(a.vi_normal_key('q'), ViOutcome::Consumed);
        assert_eq!(a.vi_normal_key('Z'), ViOutcome::Consumed);
        assert_eq!(a.input, "hello");
    }

    #[test]
    fn leaving_insert_pulls_the_cursor_back_one() {
        let mut a = vi_app("abc");
        a.composer_mode = ComposerMode::Insert;
        a.cursor = 3;
        a.vi_enter_normal();
        assert_eq!(a.composer_mode, ComposerMode::Normal);
        assert_eq!(a.cursor, 2, "vi leaves the cursor on the last char");
    }

    /// Without vi mode enabled the composer must behave exactly as before.
    #[test]
    fn normal_mode_is_inert_when_vi_is_off() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "abc".into();
        app.cursor = 3;
        app.vi_enter_normal();
        assert!(!app.in_normal_mode());
        assert_eq!(app.cursor, 3, "cursor moved with vi mode off");
    }

    /// Multi-byte text must not panic or split a character.
    #[test]
    fn motions_respect_char_boundaries() {
        let mut a = vi_app("héllo wörld");
        a.cursor = 0;
        for _ in 0..12 {
            a.vi_normal_key('l');
        }
        assert!(a.input.is_char_boundary(a.cursor));
        for _ in 0..12 {
            a.vi_normal_key('h');
        }
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('w');
        assert!(a.input.is_char_boundary(a.cursor));
    }
}
