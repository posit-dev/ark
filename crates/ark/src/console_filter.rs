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
    /// Returns the prefix string for this pattern.
    /// No prefix is a prefix of another (e.g., `"debug: "` and `"debug at "`
    /// diverge before either is complete). `try_match_prefix` relies on this.
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
    /// Returns stdout content to emit to IOPub.
    pub fn feed(&mut self, content: &str) -> Vec<String> {
        let mut emits: Vec<String> = Vec::new();

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

    /// Process a chunk of content, returning (emitted_text, bytes_consumed)
    fn process_chunk(&mut self, content: &str) -> (Option<String>, usize) {
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
                            self.state = ConsoleFilterState::Buffering {
                                buffer: content.to_string(),
                                timestamp: Instant::now(),
                            };
                            (None, content.len())
                        },
                        PrefixMatch::None => self.emit_until_newline(content),
                    }
                } else {
                    self.emit_until_newline(content)
                }
            },

            ConsoleFilterState::Buffering { buffer, timestamp } => {
                if timestamp.elapsed() > self.timeout {
                    let emit = std::mem::take(buffer);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: emit.ends_with('\n'),
                    };
                    return (Some(emit), 0);
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
                        let emit = std::mem::take(buffer);
                        self.state = ConsoleFilterState::Passthrough {
                            at_line_start: emit.ends_with('\n'),
                        };
                        (Some(emit), content.len())
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
                    return (Some(text), 0);
                }

                // Accumulate everything until ReadConsole resolves
                buffer.push_str(content);
                (None, content.len())
            },
        }
    }

    /// Emit content up to and including the next newline, updating state
    fn emit_until_newline(&mut self, content: &str) -> (Option<String>, usize) {
        if let Some(newline_pos) = content.find('\n') {
            let (before, _) = content.split_at(newline_pos + 1);
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: true,
            };
            (Some(before.to_string()), newline_pos + 1)
        } else {
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: false,
            };
            (Some(content.to_string()), content.len())
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
    ) -> (Vec<String>, Option<DebugCallTextUpdate>) {
        let mut emits: Vec<String> = Vec::new();
        let mut debug_update: Option<DebugCallTextUpdate> = None;

        // Process current state
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => {},
            ConsoleFilterState::Buffering { buffer, .. } => {
                emits.push(buffer);
            },
            ConsoleFilterState::Filtering {
                pattern,
                buffer,
                was_debugging,
                ..
            } => {
                if was_debugging || is_browser {
                    // This is the suppression point of the filer. We extract
                    // debug info for auto-stepping and drop the rest (`pattern`
                    // and `buffer`).
                    debug_update = Some(finalize_capture(pattern, &buffer));
                } else {
                    let text = format!("{}{}", pattern.prefix(), buffer);
                    emits.push(text);
                }
            },
        }

        (emits, debug_update)
    }

    /// Check for timeout and handle state transitions.
    /// Timeout means we didn't reach ReadConsole to confirm debug output,
    /// so we emit the accumulated content back to the user.
    pub fn check_timeout(&mut self) -> Vec<String> {
        self.drain_on_timeout().into_iter().collect()
    }

    fn drain_on_timeout(&mut self) -> Option<String> {
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
    pub fn flush(&mut self) -> Vec<String> {
        self.drain().into_iter().collect()
    }

    /// Replace the current state with Passthrough and return any accumulated
    /// content. Returns `None` when already in Passthrough.
    fn drain(&mut self) -> Option<String> {
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
        Some(text)
    }
}

fn try_match_prefix(content: &str) -> PrefixMatch {
    let mut has_partial = false;

    for pattern in MatchedPattern::all() {
        let prefix = pattern.prefix();
        if content.starts_with(prefix) {
            return PrefixMatch::Full(*pattern);
        }
        if prefix.starts_with(content) && !content.is_empty() {
            has_partial = true;
        }
    }

    if has_partial {
        PrefixMatch::Partial
    } else {
        PrefixMatch::None
    }
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
    /// and whether `debug_last_line` should be reset.
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

/// Strip `exiting from:` / `debugging in:` debug messages from autoprint.
///
/// These messages reach autoprint at `n_frame=0` when returning from (or
/// entering) a debugged function called at top level.
///
/// - `debugging in:` is stripped unconditionally (no return value possible
///   when entering a function)
/// - `exiting from:` is stripped only if it appears on the last line,
///   meaning there's no user result after it. If there's content after
///   (the return value), we keep everything to avoid losing it.
///
/// Callers must only invoke this when exiting a debug session (e.g.,
/// at a browser prompt, or when `debug_was_debugging` is set).
pub fn strip_leading_debug_lines(text: &mut String) {
    if text.is_empty() {
        return;
    }

    if text.starts_with(MatchedPattern::DebuggingIn.prefix()) {
        // Entering a function, no return value possible. Strip everything.
        text.clear();
    } else if text.starts_with(MatchedPattern::ExitingFrom.prefix()) {
        // Exiting a function. Only strip if "exiting from:" is on the last
        // line, meaning there's no return value after it. Otherwise keep
        // everything (noise + result) to avoid losing user content.
        let last_line = text.lines().last().unwrap_or("");
        if last_line.starts_with(MatchedPattern::ExitingFrom.prefix()) {
            text.clear();
        }
        // Otherwise keep entire buffer as-is
    }
}

/// Truncate autoprint at the first line matching any debug prefix.
///
/// Only safe at browser prompts, because prefixes like `Called from:` or
/// `debug: ` could appear in user print methods at top level.
///
/// Debug prefixes that reach autoprint at browser prompts:
/// - `"Called from: ...\n"` from `browser()` at top level
/// - `"debug at #N: <PrintValue>\n"` from stepping through braced
///   expressions like `{ browser(); 1; 2 }` at top level, possibly
///   multi-line
///
/// These always appear AFTER any user output in the same expression,
/// so truncating at the first match also removes multi-line `PrintValue`
/// continuations that don't start with a prefix themselves.
///
/// Note: `exiting from:` and `debugging in:` are NOT matched here because
/// they are handled by `strip_leading_debug_lines`. Including them would
/// incorrectly truncate when that function intentionally keeps noise to
/// preserve a return value.
pub fn truncate_at_debug_prefix(text: &mut String) {
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        // Only match prefixes that appear AFTER user output at browser prompts.
        // `exiting from:` and `debugging in:` appear BEFORE user output and
        // are handled separately by `strip_leading_debug_lines`.
        let is_debug = line.starts_with(MatchedPattern::CalledFrom.prefix()) ||
            line.starts_with(MatchedPattern::DebugAt.prefix()) ||
            line.starts_with(MatchedPattern::Debug.prefix());
        if is_debug {
            text.truncate(pos);
            return;
        }
        pos += line.len();
    }
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
        assert_eq!(emits[0], "Hello, world!\n");
    }

    #[test]
    fn test_buffering_state_on_partial_match() {
        let mut filter = ConsoleFilter::new();

        // Start with partial match - should buffer
        let emits = filter.feed("Called ");
        // While buffering, nothing emitted yet
        assert!(emits.is_empty() || emits.iter().all(|s| s.is_empty()));
    }

    #[test]
    fn test_non_matching_prefix_emitted() {
        let mut filter = ConsoleFilter::new();

        // Content that starts like a prefix but doesn't match
        let emits = filter.feed("Calling function...\n");

        // "Calling" doesn't match any prefix, should be emitted
        let emitted: String = emits.iter().map(|s| s.as_str()).collect();

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
        assert_eq!(emits[0], "Called ");
        assert!(update.is_none());

        // After on_read_console, filter should be in Passthrough state
        let emits = filter.feed("Hello\n");
        let emitted: String = emits.iter().map(|s| s.as_str()).collect();

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
        assert_eq!(flushed[0], "Called ");
    }

    fn collect_emitted(emits: &[String]) -> String {
        emits.iter().map(|s| s.as_str()).collect()
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
        assert_eq!(emits[0], "debug: ");
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
        assert_eq!(emits[0], "debug");
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
            assert_eq!(emits[0], prefix.prefix());
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
        assert_eq!(emits[0], "debug at not-a-path\n");
    }

    #[test]
    fn test_adversarial_prefix_mid_line_passes_through() {
        // Prefix text that doesn't start at a line boundary is not filtered
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("foo debug: bar\n");

        assert_eq!(collect_emitted(&emits), "foo debug: bar\n");
    }

    #[test]
    fn test_adversarial_partial_prefix_then_non_matching() {
        // "Cal" looks like start of "Called from: " but next chunk is "culator"
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Cal");
        assert!(emits.is_empty());

        let emits = filter.feed("culator\n");
        assert_eq!(collect_emitted(&emits), "Calculator\n");
    }

    #[test]
    fn test_adversarial_flush_recovers_filtering_state() {
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let flushed = filter.flush();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0], "Called from: ");
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
        assert_eq!(emits[0], "Called from: ");
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
        let emitted = collect_emitted(&emits);
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
        let emitted = collect_emitted(&emits);
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
    fn test_on_read_console_was_debugging_toplevel_suppresses() {
        // `was_debugging=true` means the prefix was matched during a debug
        // session. Even at a top-level prompt (is_browser=false), the content
        // is suppressed (e.g., `exiting from:` followed by top-level prompt).
        let mut filter = ConsoleFilter::new();
        filter.set_debugging(true);
        let emits = filter.feed("Called from: top level\n");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(false);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_on_read_console_was_debugging_browser_suppresses() {
        // Both `was_debugging` and `is_browser` true: suppressed.
        let mut filter = ConsoleFilter::new();
        filter.set_debugging(true);
        let emits = filter.feed("Called from: top level\n");
        assert!(emits.is_empty());

        let (emits, update) = filter.on_read_console(true);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_filtering_accumulates_across_feeds() {
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let emits = filter.feed("some ");
        assert!(emits.is_empty());
        let emits = filter.feed("more content\n");
        assert!(emits.is_empty());

        let (emits, _) = filter.on_read_console(false);
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0], "Called from: some more content\n");
    }

    // --- `truncate_at_debug_prefix` tests (browser-only path) ---

    #[test]
    fn test_truncate_at_first_debug_prefix() {
        let mut text = String::from(
            "[1] 42\n\
             normal output\n\
             debug at file.R#1: x <- 1\n\
             [1] 42\n\
             Called from: top level\n",
        );
        truncate_at_debug_prefix(&mut text);
        assert_eq!(text, "[1] 42\nnormal output\n");
    }

    #[test]
    fn test_truncate_multiline_printvalue() {
        let mut text = String::from(
            "debug at #2: [1] 1 2 3\n\
             [4] 4 5 6\n",
        );
        truncate_at_debug_prefix(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_truncate_empty() {
        let mut text = String::new();
        truncate_at_debug_prefix(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_truncate_no_prefixes() {
        let mut text = String::from("[1] 42\nhello\n");
        truncate_at_debug_prefix(&mut text);
        assert_eq!(text, "[1] 42\nhello\n");
    }

    // --- `strip_leading_debug_lines` tests ---

    #[test]
    fn test_strip_exiting_from_with_result() {
        // When there's a return value after "exiting from:", keep everything
        // (noise + result) to avoid losing user content.
        let mut text = String::from(
            "exiting from: identity()\n\
             [1] 1\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "exiting from: identity()\n[1] 1\n");
    }

    #[test]
    fn test_strip_exiting_from_no_result() {
        // When "exiting from:" is on the last line (no result after),
        // strip everything.
        let mut text = String::from("exiting from: f()\n");
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_exiting_from_multiline_no_result() {
        // Multi-line debug message. Last line doesn't start with
        // "exiting from:", so we keep everything (conservative).
        let mut text = String::from(
            "exiting from: f(very_long_argument_name_1 = 1, very_long_argument_name_2 = 2,\n\
             \x20   very_long_argument_name_3 = 3)\n",
        );
        strip_leading_debug_lines(&mut text);
        // Last line is the continuation, not "exiting from:", so kept
        assert_eq!(
            text,
            "exiting from: f(very_long_argument_name_1 = 1, very_long_argument_name_2 = 2,\n\
             \x20   very_long_argument_name_3 = 3)\n"
        );
    }

    #[test]
    fn test_strip_exiting_from_multiline_with_result() {
        // Multi-line debug message followed by result. Keep everything.
        let mut text = String::from(
            "exiting from: f(very_long_argument_name_1 = 1, very_long_argument_name_2 = 2,\n\
             \x20   very_long_argument_name_3 = 3)\n\
             [1] 42\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(
            text,
            "exiting from: f(very_long_argument_name_1 = 1, very_long_argument_name_2 = 2,\n\
             \x20   very_long_argument_name_3 = 3)\n\
             [1] 42\n"
        );
    }

    #[test]
    fn test_strip_debugging_in_clears_all() {
        // "debugging in:" fires when entering a function, no return value
        // is possible. Strip everything unconditionally.
        let mut text = String::from("debugging in: f()\n");
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_debugging_in_multiline_clears_all() {
        // Multi-line "debugging in:" is also cleared entirely.
        let mut text = String::from(
            "debugging in: f(very_long_argument_name_1 = 1, very_long_argument_name_2 = 2,\n\
             \x20   very_long_argument_name_3 = 3)\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_debugging_in_with_following_content_clears_all() {
        // Even if there's content after "debugging in:", it's debug noise
        // (like "debug at"), not a return value. Clear everything.
        let mut text = String::from(
            "debugging in: f()\n\
             debug at file.R#1: x <- 1\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_does_not_touch_non_leading() {
        // Buffer doesn't start with debug prefix, leave it alone.
        let mut text = String::from(
            "[1] 42\n\
             exiting from: f()\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "[1] 42\nexiting from: f()\n");
    }

    #[test]
    fn test_strip_exiting_from_result_ends_with_paren() {
        // User's result ends with ')'. Last line doesn't start with
        // "exiting from:", so we keep everything.
        let mut text = String::from(
            "exiting from: f()\n\
             list(a = 1)\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "exiting from: f()\nlist(a = 1)\n");
    }

    #[test]
    fn test_strip_exiting_from_multiple_with_result() {
        // Multiple "exiting from:" messages followed by result. Keep all.
        let mut text = String::from(
            "exiting from: g()\n\
             exiting from: f()\n\
             [1] 1\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "exiting from: g()\nexiting from: f()\n[1] 1\n");
    }

    #[test]
    fn test_strip_exiting_from_multiple_last_line() {
        // Multiple "exiting from:" messages, last line is "exiting from:".
        // This means no result, so clear everything.
        let mut text = String::from(
            "exiting from: g()\n\
             exiting from: f()\n",
        );
        strip_leading_debug_lines(&mut text);
        assert_eq!(text, "");
    }
}
