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
        }
    }

    fn all() -> &'static [MatchedPattern] {
        &[
            MatchedPattern::CalledFrom,
            MatchedPattern::DebugAt,
            MatchedPattern::Debug,
        ]
    }
}

/// State of the stream filter state machine
enum ConsoleFilterState {
    /// Default state. Content is emitted to IOPub immediately.
    Passthrough {
        /// Whether the last character emitted was `\n`
        at_line_start: bool,
    },

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
    /// Whether to suppress confirmed debug output. When `false`, the
    /// filter still extracts debug info for auto-stepping but emits
    /// the content instead of dropping it.
    suppress: bool,
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
            suppress: true,
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
            suppress: true,
        }
    }

    pub fn set_debugging(&mut self, is_debugging: bool) {
        self.is_debugging = is_debugging;
    }

    pub fn set_suppress(&mut self, suppress: bool) {
        self.suppress = suppress;
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
                    // At line boundary, check if content matches a prefix
                    match try_match_prefix(content) {
                        Some(pattern) => {
                            let prefix_len = pattern.prefix().len();
                            self.state = ConsoleFilterState::Filtering {
                                pattern,
                                buffer: String::new(),
                                timestamp: Instant::now(),
                                was_debugging: self.is_debugging,
                            };
                            (None, prefix_len)
                        },
                        None => self.emit_until_newline(content),
                    }
                } else {
                    self.emit_until_newline(content)
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

                // At line boundaries within a captured debug message, check
                // for nested prefixes. R emits each prefix as a complete
                // WriteConsole call so we can rely on full prefixes appearing
                // at the start of new content.
                let at_line_boundary = buffer.is_empty() || buffer.ends_with('\n');
                if at_line_boundary {
                    if let Some(new_pattern) = try_match_prefix(content) {
                        *pattern = new_pattern;
                        buffer.clear();
                        return (None, new_pattern.prefix().len());
                    }
                }

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
    ///   session, so it's debug machinery output.
    /// - `is_browser`: we weren't debugging but we've now landed on a
    ///   browser prompt, so this is debug-entry output like `Called from:`.
    /// - Neither: user output at top level that happened to match a
    ///   prefix -- emit it.
    pub fn on_read_console(
        &mut self,
        is_browser: bool,
    ) -> (Option<String>, Option<DebugCallTextUpdate>) {
        let mut emit: Option<String> = None;
        let mut debug_update: Option<DebugCallTextUpdate> = None;

        // Process current state
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => {},
            ConsoleFilterState::Filtering {
                pattern,
                buffer,
                was_debugging,
                ..
            } => {
                if was_debugging || is_browser {
                    debug_update = finalize_capture(pattern, &buffer);
                }
                if !(was_debugging || is_browser) || !self.suppress {
                    let text = format!("{}{}", pattern.prefix(), buffer);
                    emit = Some(text);
                }
            },
        }

        (emit, debug_update)
    }

    /// Check for timeout and handle state transitions.
    /// Timeout means we didn't reach ReadConsole to confirm debug output,
    /// so we emit the accumulated content back to the user.
    pub fn check_timeout(&mut self) -> Option<String> {
        self.drain_on_timeout()
    }

    fn drain_on_timeout(&mut self) -> Option<String> {
        let timed_out = match &self.state {
            ConsoleFilterState::Passthrough { .. } => false,
            ConsoleFilterState::Filtering { timestamp, .. } => timestamp.elapsed() > self.timeout,
        };
        if timed_out {
            self.drain()
        } else {
            None
        }
    }

    /// Get any buffered content that should be emitted (for cleanup)
    pub fn flush(&mut self) -> Option<String> {
        self.drain()
    }

    /// Replace the current state with Passthrough and return any accumulated
    /// content. Returns `None` when already in Passthrough.
    fn drain(&mut self) -> Option<String> {
        let prev = std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        });
        let text = match prev {
            ConsoleFilterState::Passthrough { .. } => return None,
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

fn try_match_prefix(content: &str) -> Option<MatchedPattern> {
    for pattern in MatchedPattern::all() {
        if content.starts_with(pattern.prefix()) {
            return Some(*pattern);
        }
    }
    None
}

/// Update to apply to the Console's debug_call_text field
#[derive(Debug)]
pub enum DebugCallTextUpdate {
    /// Set to Finalized with the given text and kind
    Finalized(String, DebugCallTextKind),
}

/// Finalize a captured debug message and produce the appropriate debug state
/// update. Nested prefixes are already resolved during accumulation (in
/// `process_chunk`), so `pattern` always reflects the last prefix seen.
fn finalize_capture(pattern: MatchedPattern, buffer: &str) -> Option<DebugCallTextUpdate> {
    match pattern {
        MatchedPattern::DebugAt => Some(extract_debug_at_update(buffer)),
        MatchedPattern::Debug => Some(extract_debug_update(buffer)),
        MatchedPattern::CalledFrom => None,
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
            continue;
        }
        if in_digits {
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

/// Strip `Called from:` / `debug at` / `debug:` lines from autoprint.
///
/// These prefixes appear after user output (e.g. `{ browser(); 1; 2 }`
/// at top level produces user output followed by `debug at` lines).
/// Truncating at the first match also removes multi-line `PrintValue`
/// continuations.
///
/// Must only be called at browser prompts: at top level, user print
/// methods could legitimately produce output matching these prefixes.
pub fn strip_step_lines(text: &mut String) {
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
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
        assert_eq!(
            try_match_prefix("Called from: "),
            Some(MatchedPattern::CalledFrom)
        );
        assert_eq!(try_match_prefix("debug at "), Some(MatchedPattern::DebugAt));
        assert_eq!(try_match_prefix("debug: "), Some(MatchedPattern::Debug));

        // No longer filtered: "debugging in:" and "exiting from:" pass through
        assert_eq!(try_match_prefix("debugging in: "), None);
        assert_eq!(try_match_prefix("exiting from: "), None);

        // Partial content does not match
        assert_eq!(try_match_prefix("Cal"), None);
        assert_eq!(try_match_prefix("debug"), None);
        assert_eq!(try_match_prefix("debug "), None);
        assert_eq!(try_match_prefix("debugging "), None);

        // Non-matches
        assert_eq!(try_match_prefix("Hello"), None);
        assert_eq!(try_match_prefix("call from"), None);
        assert_eq!(try_match_prefix(""), None);
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
    fn test_non_matching_prefix_emitted() {
        let mut filter = ConsoleFilter::new();

        // Content that starts like a prefix but doesn't match
        let emits = filter.feed("Calling function...\n");

        // "Calling" doesn't match any prefix, should be emitted
        let emitted: String = emits.iter().map(|s| s.as_str()).collect();

        assert!(emitted.contains("Calling"));
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

        let emit = filter.check_timeout();
        assert_eq!(emit.unwrap(), "debug: ");
    }

    #[test]
    fn test_adversarial_all_prefixes_timeout_emit() {
        for prefix in MatchedPattern::all() {
            let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
            let emits = filter.feed(prefix.prefix());
            assert!(emits.is_empty());

            std::thread::sleep(Duration::from_millis(5));

            let emit = filter.check_timeout();
            assert_eq!(emit.unwrap(), prefix.prefix());
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

        let emit = filter.check_timeout();
        assert_eq!(emit.unwrap(), "debug at not-a-path\n");
    }

    #[test]
    fn test_adversarial_prefix_mid_line_passes_through() {
        // Prefix text that doesn't start at a line boundary is not filtered
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("foo debug: bar\n");

        assert_eq!(collect_emitted(&emits), "foo debug: bar\n");
    }

    #[test]
    fn test_adversarial_partial_prefix_passes_through() {
        // "Cal" doesn't fully match any prefix, so it's emitted immediately
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Cal");
        assert_eq!(collect_emitted(&emits), "Cal");

        let emits = filter.feed("culator\n");
        assert_eq!(collect_emitted(&emits), "culator\n");
    }

    #[test]
    fn test_adversarial_flush_recovers_filtering_state() {
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let flushed = filter.flush();
        assert_eq!(flushed.unwrap(), "Called from: ");
    }

    #[test]
    fn test_adversarial_on_read_console_browser_suppresses() {
        // Content in Filtering state IS suppressed when a browser prompt
        // arrives, because that confirms it was a real debug message.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("debug: x + 1\n");
        assert!(emits.is_empty());

        let (emit, update) = filter.on_read_console(true);
        assert!(emit.is_none());
        assert!(update.is_some());
    }

    #[test]
    fn test_adversarial_on_read_console_toplevel_emits() {
        // Content in Filtering state IS emitted when a top-level prompt
        // arrives, because that means it was user output matching a prefix.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("Called from: ");
        assert!(emits.is_empty());

        let (emit, update) = filter.on_read_console(false);
        assert_eq!(emit.unwrap(), "Called from: ");
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
        filter.feed("Called from: ");

        std::thread::sleep(Duration::from_millis(5));

        let emits = filter.feed("next line\n");
        let emitted = collect_emitted(&emits);
        assert!(emitted.contains("Called from: "));
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

        let (emit, update) = filter.on_read_console(true);
        assert!(emit.is_none());
        assert!(update.is_some());
    }

    #[test]
    fn test_debugging_in_passes_through_debug_at_filtered() {
        // "debugging in:" is no longer filtered, so it passes through
        // immediately. "debug at" is still filtered.
        let mut filter = ConsoleFilter::new();
        let emits = filter.feed("debugging in: f()\n");
        assert_eq!(collect_emitted(&emits), "debugging in: f()\n");

        let emits = filter.feed("debug at file.R#1: x <- 1\n");
        assert!(emits.is_empty());

        let (emit, update) = filter.on_read_console(true);
        assert!(emit.is_none());
        assert!(update.is_some());
    }

    #[test]
    fn test_exiting_from_passes_through() {
        // "exiting from:" is no longer filtered, it passes through immediately.
        let mut filter = ConsoleFilter::new();
        filter.set_debugging(true);
        let emits = filter.feed("exiting from: f()\n");
        assert_eq!(collect_emitted(&emits), "exiting from: f()\n");
    }

    #[test]
    fn test_debugging_in_passes_through() {
        // "debugging in:" is no longer filtered, it passes through immediately.
        let mut filter = ConsoleFilter::new();
        filter.set_debugging(true);
        let emits = filter.feed("debugging in: f()\n");
        assert_eq!(collect_emitted(&emits), "debugging in: f()\n");
    }

    #[test]
    fn test_suppress_false_emits_and_extracts() {
        // With suppress disabled, debug output is emitted but debug info
        // is still extracted for auto-stepping.
        let mut filter = ConsoleFilter::new();
        filter.set_suppress(false);
        filter.set_debugging(true);
        let emits = filter.feed("debug at file.R#10: x <- 1\n");
        assert!(emits.is_empty());

        let (emit, update) = filter.on_read_console(true);
        assert_eq!(emit.unwrap(), "debug at file.R#10: x <- 1\n");
        assert!(update.is_some());
    }

    #[test]
    fn test_suppress_false_entry_exit_passes_through() {
        // "exiting from:" is no longer filtered regardless of suppress flag.
        let mut filter = ConsoleFilter::new();
        filter.set_suppress(false);
        filter.set_debugging(true);
        let emits = filter.feed("exiting from: f()\n");
        assert_eq!(collect_emitted(&emits), "exiting from: f()\n");
    }

    #[test]
    fn test_suppress_false_non_debug_still_emits() {
        // Non-debug output at top level: emitted regardless of suppress flag.
        let mut filter = ConsoleFilter::new();
        filter.set_suppress(false);
        let emits = filter.feed("Called from: user output\n");
        assert!(emits.is_empty());

        let (emit, update) = filter.on_read_console(false);
        assert_eq!(emit.unwrap(), "Called from: user output\n");
        assert!(update.is_none());
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

        let (emit, _) = filter.on_read_console(false);
        assert_eq!(emit.unwrap(), "Called from: some more content\n");
    }

    // --- `strip_step_lines` tests (browser-only path) ---

    #[test]
    fn test_strip_step_at_first_debug_prefix() {
        let mut text = String::from(
            "[1] 42\n\
             normal output\n\
             debug at file.R#1: x <- 1\n\
             [1] 42\n\
             Called from: top level\n",
        );
        strip_step_lines(&mut text);
        assert_eq!(text, "[1] 42\nnormal output\n");
    }

    #[test]
    fn test_strip_step_multiline_printvalue() {
        let mut text = String::from(
            "debug at #2: [1] 1 2 3\n\
             [4] 4 5 6\n",
        );
        strip_step_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_step_empty() {
        let mut text = String::new();
        strip_step_lines(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn test_strip_step_no_prefixes() {
        let mut text = String::from("[1] 42\nhello\n");
        strip_step_lines(&mut text);
        assert_eq!(text, "[1] 42\nhello\n");
    }
}
