//
// console_filter.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

// Filter for debug console output. Removes R's internal debug messages from
// user-visible console output while preserving the information needed for
// auto-stepping.
//
// R's debug handler emits messages like `debug at file.R#10: ` followed by
// `PrintValue(expr)` which renders the expression about to be evaluated.
// For calls and symbols, `PrintValue` output happens to look like R code,
// but for literals `PrintValue(1)` produces `[1] 1` which is not valid R
// syntax. This makes parse-based detection of "complete debug messages"
// unreliable.
//
// Instead, we rely on the fact that R's debug handler always calls
// `ReadConsole` (via `do_browser`) after emitting debug messages, and no
// user code runs in between. So everything emitted between a debug prefix
// and the next `ReadConsole` is guaranteed to be debug output. We
// accumulate all of it in this filter managed by `WriteConsole` and decide at
// `ReadConsole` time whether to suppress or emit.

use std::time::Duration;
use std::time::Instant;

use amalthea::wire::stream::Stream;

use crate::console::DebugCallText;
use crate::console::DebugCallTextKind;

/// Patterns to filter from console output.
/// Each pattern is matched at a line boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MatchedPattern {
    /// `Called from: <expr>` - emitted by browser() before entering debug mode
    CalledFrom,
    /// `debug at <path>#<line>: <expr>` - emitted when stepping with srcrefs
    DebugAt,
    /// `debug: <expr>` - emitted when stepping without srcrefs
    Debug,
    /// `debugging in: <expr>` - emitted when entering a closure
    DebuggingIn,
    /// `exiting from: <expr>` - emitted when exiting a closure
    ExitingFrom,
}

impl MatchedPattern {
    /// Returns the prefix string for this pattern
    fn prefix(&self) -> &'static str {
        match self {
            MatchedPattern::CalledFrom => "Called from: ",
            MatchedPattern::DebugAt => "debug at ",
            MatchedPattern::Debug => "debug: ",
            MatchedPattern::DebuggingIn => "debugging in: ",
            MatchedPattern::ExitingFrom => "exiting from: ",
        }
    }

    fn all() -> &'static [MatchedPattern] {
        &[
            MatchedPattern::CalledFrom,
            MatchedPattern::DebugAt,
            MatchedPattern::Debug,
            MatchedPattern::DebuggingIn,
            MatchedPattern::ExitingFrom,
        ]
    }
}

/// Result of trying to match a prefix against buffered content
enum PrefixMatch {
    /// Content fully matches a prefix
    Full(MatchedPattern),
    /// Content is a partial match of at least one prefix
    Partial,
    /// Content cannot match any prefix
    None,
}

/// State of the stream filter state machine
enum ConsoleFilterState {
    /// Default state. Content is emitted to IOPub immediately.
    Passthrough {
        /// Whether the last character emitted was `\n`
        at_line_start: bool,
    },

    /// At the start of a new line, new content partially matches a filter pattern.
    /// Accumulating in buffer, waiting for enough data to confirm or reject.
    Buffering { buffer: String, timestamp: Instant },

    /// Confirmed prefix match. Accumulating all subsequent content until
    /// `ReadConsole` resolves whether this is real debug output or adversarial
    /// user output.
    Filtering {
        pattern: MatchedPattern,
        buffer: String,
        timestamp: Instant,
        /// Whether the console was in a debug session when this match started.
        was_debugging: bool,
    },
}

/// Filter for debug console output
pub struct ConsoleFilter {
    state: ConsoleFilterState,
    timeout: Duration,
    /// Whether we're currently inside a debug session. Updated by the
    /// console so the filter can record context when entering `Filtering`.
    is_debugging: bool,
}

fn get_timeout() -> Duration {
    if stdext::IS_TESTING {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(50)
    }
}

impl ConsoleFilter {
    pub fn new() -> Self {
        Self {
            state: ConsoleFilterState::Passthrough {
                at_line_start: true,
            },
            timeout: get_timeout(),
            is_debugging: false,
        }
    }

    #[cfg(test)]
    fn new_with_timeout(timeout: Duration) -> Self {
        Self {
            state: ConsoleFilterState::Passthrough {
                at_line_start: true,
            },
            timeout,
            is_debugging: false,
        }
    }

    pub fn set_debugging(&mut self, is_debugging: bool) {
        self.is_debugging = is_debugging;
    }

    /// Feed content through the filter.
    /// Returns content to emit to IOPub.
    pub fn feed(&mut self, content: &str) -> Vec<(String, Stream)> {
        let mut emits: Vec<(String, Stream)> = Vec::new();

        // Check current state timeout
        if let Some(emit) = self.drain_on_timeout() {
            emits.push(emit);
        }

        // Process content chunk by chunk to handle line boundaries
        let mut remaining = content;

        while !remaining.is_empty() {
            let (emit, consumed) = self.process_chunk(remaining);
            if let Some(emit) = emit {
                emits.push(emit);
            }
            remaining = &remaining[consumed..];
        }

        emits
    }

    /// Process a chunk of content, returning (action, bytes_consumed)
    fn process_chunk(&mut self, content: &str) -> (Option<(String, Stream)>, usize) {
        match &mut self.state {
            ConsoleFilterState::Passthrough { at_line_start } => {
                if *at_line_start {
                    // At line boundary, check if content could match a prefix
                    match try_match_prefix(content) {
                        PrefixMatch::Full(pattern) => {
                            let prefix_len = pattern.prefix().len();
                            self.state = ConsoleFilterState::Filtering {
                                pattern,
                                buffer: String::new(),
                                timestamp: Instant::now(),
                                was_debugging: self.is_debugging,
                            };
                            (None, prefix_len)
                        },
                        PrefixMatch::Partial => {
                            // Start buffering
                            self.state = ConsoleFilterState::Buffering {
                                buffer: content.to_string(),
                                timestamp: Instant::now(),
                            };
                            (None, content.len())
                        },
                        PrefixMatch::None => {
                            // Emit content up to next newline
                            self.emit_until_newline(content)
                        },
                    }
                } else {
                    // Not at line boundary, emit until we hit a newline
                    self.emit_until_newline(content)
                }
            },

            ConsoleFilterState::Buffering { buffer, timestamp } => {
                if timestamp.elapsed() > self.timeout {
                    let emit = buffer.clone();
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: emit.ends_with('\n'),
                    };
                    return (Some((emit, Stream::Stdout)), 0);
                }

                buffer.push_str(content);

                match try_match_prefix(buffer) {
                    PrefixMatch::Full(pattern) => {
                        // Full match! Extract any content after the prefix
                        let prefix_len = pattern.prefix().len();
                        let after_prefix = buffer[prefix_len..].to_string();
                        self.state = ConsoleFilterState::Filtering {
                            pattern,
                            buffer: after_prefix,
                            timestamp: Instant::now(),
                            was_debugging: self.is_debugging,
                        };
                        (None, content.len())
                    },
                    PrefixMatch::Partial => {
                        // Still partial, keep buffering
                        (None, content.len())
                    },
                    PrefixMatch::None => {
                        // Cannot match, flush buffer
                        let emit = buffer.clone();
                        self.state = ConsoleFilterState::Passthrough {
                            at_line_start: emit.ends_with('\n'),
                        };
                        (Some((emit, Stream::Stdout)), content.len())
                    },
                }
            },

            ConsoleFilterState::Filtering {
                pattern,
                buffer,
                timestamp,
                ..
            } => {
                if timestamp.elapsed() > self.timeout {
                    let text = format!("{}{}", pattern.prefix(), buffer);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: buffer.ends_with('\n'),
                    };
                    return (Some((text, Stream::Stdout)), 0);
                }

                // Accumulate everything until ReadConsole resolves
                buffer.push_str(content);
                (None, content.len())
            },
        }
    }

    /// Emit content up to and including the next newline, updating state
    fn emit_until_newline(&mut self, content: &str) -> (Option<(String, Stream)>, usize) {
        if let Some(newline_pos) = content.find('\n') {
            let (before, _) = content.split_at(newline_pos + 1);
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: true,
            };
            (Some((before.to_string(), Stream::Stdout)), newline_pos + 1)
        } else {
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: false,
            };
            (Some((content.to_string(), Stream::Stdout)), content.len())
        }
    }

    /// Called when ReadConsole is entered, flush/finalize any pending state.
    ///
    /// The `is_browser` parameter indicates whether this is a browser prompt
    /// (debug mode) or a top-level prompt (normal execution). This is the key
    /// signal for distinguishing real debug messages from adversarial user
    /// output:
    ///
    /// When `was_debugging || is_browser`, the content is suppressed as
    /// real debug output. Otherwise it is emitted back to the user.
    ///
    /// - `was_debugging`: the prefix was matched while already in a debug
    ///   session, so it's debug machinery output (including `exiting from:`
    ///   which may be followed by a top-level prompt).
    /// - `is_browser`: we weren't debugging but we've now landed on a
    ///   browser prompt, so this is debug-entry output like `Called from:`.
    /// - Neither: user output at top level that happened to match a
    ///   prefix — emit it.
    ///
    /// Content in `Buffering` state (unconfirmed prefix match) is always
    /// emitted since we never confirmed a full prefix match.
    pub fn on_read_console(
        &mut self,
        is_browser: bool,
    ) -> (Vec<(String, Stream)>, Option<DebugCallTextUpdate>) {
        let mut emits: Vec<(String, Stream)> = Vec::new();
        let mut debug_update: Option<DebugCallTextUpdate> = None;

        // Process current state
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => {},
            ConsoleFilterState::Buffering { buffer, .. } => {
                emits.push((buffer, Stream::Stdout));
            },
            ConsoleFilterState::Filtering {
                pattern,
                buffer,
                was_debugging,
                ..
            } => {
                if was_debugging || is_browser {
                    debug_update = Some(finalize_capture(pattern, &buffer));
                } else {
                    let text = format!("{}{}", pattern.prefix(), buffer);
                    emits.push((text, Stream::Stdout));
                }
            },
        }

        (emits, debug_update)
    }

    /// Check for timeout and handle state transitions.
    /// Timeout means we didn't reach ReadConsole to confirm debug output,
    /// so we emit the accumulated content back to the user.
    pub fn check_timeout(&mut self) -> Vec<(String, Stream)> {
        self.drain_on_timeout().into_iter().collect()
    }

    fn drain_on_timeout(&mut self) -> Option<(String, Stream)> {
        let timed_out = match &self.state {
            ConsoleFilterState::Passthrough { .. } => false,
            ConsoleFilterState::Buffering { timestamp, .. } |
            ConsoleFilterState::Filtering { timestamp, .. } => timestamp.elapsed() > self.timeout,
        };
        if timed_out {
            self.drain()
        } else {
            None
        }
    }

    /// Get any buffered content that should be emitted (for cleanup)
    pub fn flush(&mut self) -> Vec<(String, Stream)> {
        self.drain().into_iter().collect()
    }

    /// Replace the current state with Passthrough and return any accumulated
    /// content. Returns `None` when already in Passthrough.
    fn drain(&mut self) -> Option<(String, Stream)> {
        let prev = std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        });
        let text = match prev {
            ConsoleFilterState::Passthrough { .. } => return None,
            ConsoleFilterState::Buffering { buffer, .. } => buffer,
            ConsoleFilterState::Filtering {
                pattern, buffer, ..
            } => {
                format!("{}{}", pattern.prefix(), buffer)
            },
        };
        self.state = ConsoleFilterState::Passthrough {
            at_line_start: text.ends_with('\n'),
        };
        Some((text, Stream::Stdout))
    }
}

/// Try to match content against known prefixes
fn try_match_prefix(content: &str) -> PrefixMatch {
    let mut best_match: Option<MatchedPattern> = None;
    let mut best_len: usize = 0;
    let mut has_partial = false;

    // Although not necessary right now, use longest match approach to be
    // defensive
    for pattern in MatchedPattern::all() {
        let prefix = pattern.prefix();
        if content.starts_with(prefix) && prefix.len() > best_len {
            best_match = Some(*pattern);
            best_len = prefix.len();
        }
        if prefix.starts_with(content) && !content.is_empty() {
            has_partial = true;
        }
    }

    if let Some(pattern) = best_match {
        PrefixMatch::Full(pattern)
    } else if has_partial {
        PrefixMatch::Partial
    } else {
        PrefixMatch::None
    }
}

/// Find the start of the expression in a `debug at` buffer
/// The buffer contains content after `debug at `, i.e., `<path>#<line>: <expr>`
fn find_debug_at_expression_start(buffer: &str) -> Option<usize> {
    // Look for `#<digits>: ` pattern
    let mut in_digits = false;
    let bytes = buffer.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' {
            in_digits = true;
        } else if in_digits {
            if b.is_ascii_digit() {
                // Still in digits
            } else if b == b':' {
                // Found colon after digits, check for space
                if i + 2 <= buffer.len() && bytes.get(i + 1) == Some(&b' ') {
                    return Some(i + 2);
                }
                in_digits = false;
            } else {
                in_digits = false;
            }
        }
    }

    None
}

/// Update to apply to the Console's debug_call_text field
#[derive(Debug)]
pub enum DebugCallTextUpdate {
    /// Set to Finalized with the given text and kind
    Finalized(String, DebugCallTextKind),
    /// Reset debug state (for debugging in/exiting from)
    Reset,
}

impl DebugCallTextUpdate {
    /// Apply this update to a DebugCallText, returning the new value
    /// and whether debug_last_line should be reset
    pub fn apply(self) -> (DebugCallText, bool) {
        match self {
            DebugCallTextUpdate::Finalized(text, kind) => {
                (DebugCallText::Finalized(text, kind), false)
            },
            DebugCallTextUpdate::Reset => (DebugCallText::None, true),
        }
    }
}

/// Finalize a captured debug message and produce the appropriate debug state
/// update. Scans the accumulated buffer for the last `debug at ` or `debug: `
/// occurrence to extract the expression for auto-stepping.
fn finalize_capture(pattern: MatchedPattern, buffer: &str) -> DebugCallTextUpdate {
    let last_debug_at = buffer.rfind(MatchedPattern::DebugAt.prefix());
    let last_debug = buffer.rfind(MatchedPattern::Debug.prefix());

    match (last_debug_at, last_debug) {
        (Some(at_pos), Some(d_pos)) => {
            if at_pos > d_pos {
                extract_debug_at_update(&buffer[at_pos + MatchedPattern::DebugAt.prefix().len()..])
            } else {
                extract_debug_update(&buffer[d_pos + MatchedPattern::Debug.prefix().len()..])
            }
        },
        (Some(at_pos), None) => {
            extract_debug_at_update(&buffer[at_pos + MatchedPattern::DebugAt.prefix().len()..])
        },
        (None, Some(d_pos)) => {
            extract_debug_update(&buffer[d_pos + MatchedPattern::Debug.prefix().len()..])
        },
        (None, None) => {
            // No nested debug message; handle based on the initial pattern
            match pattern {
                MatchedPattern::DebugAt => extract_debug_at_update(buffer),
                MatchedPattern::Debug => extract_debug_update(buffer),
                MatchedPattern::CalledFrom => {
                    DebugCallTextUpdate::Finalized(buffer.to_string(), DebugCallTextKind::Debug)
                },
                MatchedPattern::DebuggingIn | MatchedPattern::ExitingFrom => {
                    DebugCallTextUpdate::Reset
                },
            }
        },
    }
}

fn extract_debug_at_update(after_prefix: &str) -> DebugCallTextUpdate {
    let expr = match find_debug_at_expression_start(after_prefix) {
        Some(start) => after_prefix[start..].to_string(),
        None => String::new(),
    };
    DebugCallTextUpdate::Finalized(expr, DebugCallTextKind::DebugAt)
}

fn extract_debug_update(after_prefix: &str) -> DebugCallTextUpdate {
    DebugCallTextUpdate::Finalized(after_prefix.to_string(), DebugCallTextKind::Debug)
}

/// Strip lines starting with debug prefixes from a string, used to clean
/// `autoprint_output` at browser prompts.
///
/// This is a simpler approach than the stream filter state machine above,
/// appropriate here because autoprint output only accumulates for the last
/// top-level expression. At browser-prompt time, the only prefix-matching
/// content is R's own debug noise (e.g., `"Called from: top level\n"`),
/// not user output.
///
/// All lines matching any debug prefix are removed entirely. The content
/// after debug prefixes is `PrintValue` output of the expression code,
/// not the evaluation result, so it should not appear as user output.
pub fn strip_debug_prefix_lines(text: &mut String) {
    if text.is_empty() {
        return;
    }
    let mut result = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let is_debug = MatchedPattern::all()
            .iter()
            .any(|p| line.starts_with(p.prefix()));
        if !is_debug {
            result.push_str(line);
        }
    }
    *text = result;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_matching() {
        assert!(matches!(
            try_match_prefix("Called from: "),
            PrefixMatch::Full(MatchedPattern::CalledFrom)
        ));
        assert!(matches!(
            try_match_prefix("debug at "),
            PrefixMatch::Full(MatchedPattern::DebugAt)
        ));
        assert!(matches!(
            try_match_prefix("debug: "),
            PrefixMatch::Full(MatchedPattern::Debug)
        ));
        assert!(matches!(
            try_match_prefix("debugging in: "),
            PrefixMatch::Full(MatchedPattern::DebuggingIn)
        ));
        assert!(matches!(
            try_match_prefix("exiting from: "),
            PrefixMatch::Full(MatchedPattern::ExitingFrom)
        ));

        // Partial matches
        assert!(matches!(try_match_prefix("Cal"), PrefixMatch::Partial));
        assert!(matches!(try_match_prefix("debug"), PrefixMatch::Partial));
        assert!(matches!(try_match_prefix("debug "), PrefixMatch::Partial));
        assert!(matches!(
            try_match_prefix("debugging "),
            PrefixMatch::Partial
        ));

        // Non-matches
        assert!(matches!(try_match_prefix("Hello"), PrefixMatch::None));
        assert!(matches!(try_match_prefix("call from"), PrefixMatch::None));
        assert!(matches!(try_match_prefix(""), PrefixMatch::None));
    }

    #[test]
    fn test_find_debug_at_expression_start() {
        assert_eq!(find_debug_at_expression_start("file.R#10: x + 1"), Some(11));
        assert_eq!(
            find_debug_at_expression_start("/path/to/file.R#123: foo()"),
            Some(21)
        );
        // Windows path with colon
        assert_eq!(
            find_debug_at_expression_start("C:/path/file.R#5: bar"),
            Some(18)
        );
        // No expression yet
        assert_eq!(find_debug_at_expression_start("file.R#10"), None);
        assert_eq!(find_debug_at_expression_start("file.R"), None);
    }

    #[test]
    fn test_normal_output_passthrough() {
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Hello, world!\n");

        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].0, "Hello, world!\n");
        assert_eq!(emits[0].1, Stream::Stdout);
    }

    #[test]
    fn test_buffering_state_on_partial_match() {
        let mut filter = ConsoleFilter::new();

        // Start with partial match - should buffer
        let emits = filter.feed("Called ");
        // While buffering, nothing emitted yet
        assert!(emits.is_empty() || emits.iter().all(|(s, _)| s.is_empty()));
    }

    #[test]
    fn test_non_matching_prefix_emitted() {
        let mut filter = ConsoleFilter::new();

        // Content that starts like a prefix but doesn't match
        let emits = filter.feed("Calling function...\n");

        // "Calling" doesn't match any prefix, should be emitted
        let emitted: String = emits
            .iter()
            .filter(|(_, s)| *s == Stream::Stdout)
            .map(|(s, _)| s.as_str())
            .collect();

        assert!(emitted.contains("Calling"));
    }

    #[test]
    fn test_on_read_console_emits_buffering() {
        let mut filter = ConsoleFilter::new();

        // Start buffering with partial match
        filter.feed("Called ");

        // Buffering is an unconfirmed prefix match, so ReadConsole emits it
        // regardless of whether it's a browser prompt or not
        let (emits, update) = filter.on_read_console(false);

        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "Called ");
        assert_eq!(*stream, Stream::Stdout);
        assert!(update.is_none());

        // After on_read_console, filter should be in Passthrough state
        let emits = filter.feed("Hello\n");
        let emitted: String = emits
            .iter()
            .filter(|(_, s)| *s == Stream::Stdout)
            .map(|(s, _)| s.as_str())
            .collect();

        assert!(emitted.contains("Hello"));
    }

    #[test]
    fn test_flush_returns_buffered_content() {
        let mut filter = ConsoleFilter::new();

        // Start buffering
        filter.feed("Called ");

        // Flush should return the buffered content
        let flushed = filter.flush();

        assert_eq!(flushed.len(), 1);
        let (text, stream) = &flushed[0];
        assert_eq!(text, "Called ");
        assert_eq!(*stream, Stream::Stdout);
    }

    fn collect_stdout(actions: &[(String, Stream)]) -> String {
        actions
            .iter()
            .filter(|(_, s)| *s == Stream::Stdout)
            .map(|(s, _)| s.as_str())
            .collect()
    }

    // --- Adversarial output tests ---
    //
    // These verify that user output resembling debug messages is recovered
    // rather than silently swallowed. The filter only suppresses content
    // that is confirmed by reaching ReadConsole (the browser prompt).

    #[test]
    fn test_adversarial_filtering_timeout_emits() {
        // Prefix matched but no ReadConsole arrives. Timeout emits
        // the accumulated content back to the user.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));

        let emits = filter.feed("debug: ");
        assert!(emits.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let emits = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "debug: ");
        assert_eq!(*stream, Stream::Stdout);
    }

    #[test]
    fn test_adversarial_buffering_timeout_emits() {
        // Partial prefix match that times out before resolving
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
        let emits = filter.feed("debug");
        assert!(emits.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let emits = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "debug");
        assert_eq!(*stream, Stream::Stdout);
    }

    #[test]
    fn test_adversarial_all_prefixes_timeout_emit() {
        for prefix in MatchedPattern::all() {
            let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
            let emits = filter.feed(prefix.prefix());
            assert!(emits.is_empty());

            std::thread::sleep(Duration::from_millis(5));

            let emits = filter.check_timeout();
            assert_eq!(emits.len(), 1);
            let (text, _) = &emits[0];
            assert_eq!(text, prefix.prefix());
        }
    }

    #[test]
    fn test_adversarial_debug_at_malformed_path_timeout_emits() {
        // "debug at " without the expected `#<digits>: ` pattern stays in
        // Filtering (no parse-based rejection). Timeout recovers content.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));

        let emits = filter.feed("debug at ");
        assert!(emits.is_empty());
        let emits = filter.feed("not-a-path\n");
        assert!(emits.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let emits = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, _) = &emits[0];
        assert_eq!(text, "debug at not-a-path\n");
    }

    #[test]
    fn test_adversarial_prefix_mid_line_passes_through() {
        // Prefix text that doesn't start at a line boundary is not filtered
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("foo debug: bar\n");

        assert_eq!(collect_stdout(&emits), "foo debug: bar\n");
    }

    #[test]
    fn test_adversarial_partial_prefix_then_non_matching() {
        // "Cal" looks like start of "Called from: " but next chunk is "culator"
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Cal");
        assert!(emits.is_empty());

        let emits = filter.feed("culator\n");
        assert_eq!(collect_stdout(&emits), "Calculator\n");
    }

    #[test]
    fn test_adversarial_flush_recovers_filtering_state() {
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let flushed = filter.flush();
        assert_eq!(flushed.len(), 1);
        let (text, stream) = &flushed[0];
        assert_eq!(text, "Called from: ");
        assert_eq!(*stream, Stream::Stdout);
    }

    #[test]
    fn test_adversarial_on_read_console_browser_suppresses() {
        // Content in Filtering state IS suppressed when a browser prompt
        // arrives, because that confirms it was a real debug message.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(true);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_adversarial_on_read_console_toplevel_emits() {
        // Content in Filtering state IS emitted when a top-level prompt
        // arrives, because that means it was user output matching a prefix.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(false);
        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "Called from: ");
        assert_eq!(*stream, Stream::Stdout);
        assert!(update.is_none());
    }

    #[test]
    fn test_adversarial_feed_after_timeout_works_normally() {
        // After a timeout recovery, subsequent output passes through normally
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
        filter.feed("debug: ");

        std::thread::sleep(Duration::from_millis(5));

        // Next feed triggers timeout recovery then processes new content
        let emits = filter.feed("normal output\n");
        let emitted = collect_stdout(&emits);
        assert!(emitted.contains("debug: "));
        assert!(emitted.contains("normal output\n"));
    }

    #[test]
    fn test_adversarial_timeout_during_feed_emits() {
        // Timeout fires inside `feed` (via check_timeout at the start)
        // when new content arrives after the deadline.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
        filter.feed("exiting from: ");

        std::thread::sleep(Duration::from_millis(5));

        let emits = filter.feed("next line\n");
        let emitted = collect_stdout(&emits);
        assert!(emitted.contains("exiting from: "));
        assert!(emitted.contains("next line\n"));
    }

    // --- Tests for accumulate-until-ReadConsole approach ---

    #[test]
    fn test_literal_debug_at_stays_filtering() {
        // `[1] 1` is not valid R syntax. The old parse-based filter would
        // flush-emit this as "not a real debug message". The new filter
        // accumulates everything until ReadConsole.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("debug at file.R#1: [1] 1\n");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(true);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_debugging_in_accumulates_debug_at() {
        // When entering a debugged function, both "debugging in:" and
        // "debug at" arrive before ReadConsole. The filter accumulates
        // everything in a single Filtering state.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("debugging in: f()\n");
        assert!(emits.is_empty());
        let emits = filter.feed("debug at file.R#1: x <- 1\n");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(true);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_strip_debug_prefix_lines_all_prefixes() {
        let mut text = String::from(
            "debug at file.R#1: x <- 1\n\
             [1] 42\n\
             debug: y\n\
             Called from: top level\n\
             debugging in: f()\n\
             exiting from: f()\n\
             normal output\n",
        );
        strip_debug_prefix_lines(&mut text);
        assert_eq!(text, "[1] 42\nnormal output\n");
    }

    #[test]
    fn test_strip_debug_prefix_lines_empty() {
        let mut text = String::new();
        strip_debug_prefix_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_debug_prefix_lines_no_prefixes() {
        let mut text = String::from("[1] 42\nhello\n");
        strip_debug_prefix_lines(&mut text);
        assert_eq!(text, "[1] 42\nhello\n");
    }
}
