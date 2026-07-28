//! Full-screen modern TUI (alt-screen ratatui pager).
//!
//! The only interactive surface. See `docs/tui/README.md`.

mod anim;
mod app;
mod colors;
mod diffview;
#[cfg(test)]
mod fake_engine;
mod layout;
mod markdown;
mod mentions;
mod modal;
mod mode;
mod model_picker;
mod palette;
mod render;
pub mod resume_state;
mod run;
mod scroll;
mod search;
mod session_picker;
mod session_views;
pub mod session_work;
mod sink;
#[cfg(test)]
mod snapshot;
mod stream_buffer;
mod tasks;
mod terminal_caps;
pub mod theme_picker;
mod toolcard;
mod vi;

pub use run::{CliPermissionOverride, run_modern_tui};

/// Why the interactive TUI cannot start, or `None` when it can.
///
/// The TUI takes over the terminal (raw mode + alt screen), which needs a
/// real TTY on **both** ends: stdin for key events, stdout for the screen.
/// Without the check `enable_raw_mode()` fails deep inside setup and the
/// user sees a bare OS errno, so this turns the failure into a message
/// that names the non-interactive flags instead.
pub fn non_interactive_reason(stdin_is_tty: bool, stdout_is_tty: bool) -> Option<String> {
    let stream = match (stdin_is_tty, stdout_is_tty) {
        (true, true) => return None,
        (false, false) => "stdin and stdout are",
        (false, true) => "stdin is",
        (true, false) => "stdout is",
    };
    Some(format!(
        "the interactive TUI needs a terminal, but {stream} not one \
         (piped, redirected, or running under CI).\n\
         For non-interactive use run a single prompt instead:\n  \
         agent --prompt \"…\"   (short: -p)\n  \
         agent --prompt \"…\" --output-format json   (JSONL events on stdout)"
    ))
}

/// Live form of [`non_interactive_reason`], probing the real streams.
pub fn non_interactive_reason_from_env() -> Option<String> {
    use std::io::IsTerminal;
    non_interactive_reason(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

#[cfg(test)]
mod tests {
    use super::non_interactive_reason;

    #[test]
    fn both_streams_tty_is_allowed() {
        assert!(non_interactive_reason(true, true).is_none());
    }

    #[test]
    fn piped_stdin_is_rejected_and_names_the_stream() {
        let msg = non_interactive_reason(false, true).expect("must refuse");
        assert!(msg.contains("stdin is not one"), "{msg}");
    }

    #[test]
    fn piped_stdout_is_rejected_and_names_the_stream() {
        let msg = non_interactive_reason(true, false).expect("must refuse");
        assert!(msg.contains("stdout is not one"), "{msg}");
    }

    #[test]
    fn fully_piped_names_both_streams() {
        let msg = non_interactive_reason(false, false).expect("must refuse");
        assert!(msg.contains("stdin and stdout are not one"), "{msg}");
    }

    #[test]
    fn message_points_at_the_non_interactive_flags() {
        // The whole point of the guard is actionability: every refusal
        // must name --prompt/-p and the JSON output format.
        for msg in [
            non_interactive_reason(false, true),
            non_interactive_reason(true, false),
            non_interactive_reason(false, false),
        ] {
            let msg = msg.expect("must refuse");
            assert!(msg.contains("--prompt"), "{msg}");
            assert!(msg.contains("-p"), "{msg}");
            assert!(msg.contains("--output-format json"), "{msg}");
        }
    }
}
