//
// console_filter.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Filter for debug console output. Removes R's internal debug messages from
// user-visible console output while preserving the information needed for
// auto-stepping.
//

use std::env;
use std::time::Duration;
use std::time::Instant;

use amalthea::wire::stream::Stream;
use harp::parse::parse_status;
use harp::parse::ParseInput;
use harp::parse::ParseResult;

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
    Buffering {
        buffer: String,
        stream: Stream,
        timestamp: Instant,
    },

    /// Confirmed match. Accumulating the expression that follows the prefix.
    /// Content is suppressed from IOPub.
    Filtering {
        pattern: MatchedPattern,
        expr_buffer: String,
        timestamp: Instant,
    },
}

/// Filter for debug console output
pub struct ConsoleFilter {
    state: ConsoleFilterState,
    timeout: Duration,
}

fn get_timeout() -> Duration {
    // Allow override via environment variable for testing
    if let Ok(ms) = env::var("ARK_STREAM_FILTER_TIMEOUT_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }

    // Use longer timeout in tests since CI can be slow
    if std::env::var("IS_TESTING").is_ok() {
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
        }
    }

    #[cfg(test)]
    fn new_with_timeout(timeout: Duration) -> Self {
        Self {
            state: ConsoleFilterState::Passthrough {
                at_line_start: true,
            },
            timeout,
        }
    }

    /// Feed content through the filter and get actions to perform.
    /// Returns actions to emit content to IOPub.
    /// Also returns the captured DebugCallText state update, if any.
    pub fn feed(
        &mut self,
        content: &str,
        stream: Stream,
    ) -> (Vec<(String, Stream)>, Option<DebugCallTextUpdate>) {
        // Only filter stdout as debug messages are emitted on stdout
        if stream == Stream::Stderr {
            return (vec![(content.to_string(), stream)], None);
        }

        let (timeout_emit, timeout_update) = self.check_timeout_internal();

        let mut actions: Vec<(String, Stream)> = Vec::new();
        if let Some(emit) = timeout_emit {
            actions.push(emit);
        }
        let mut debug_update = timeout_update;

        // Process content character by character to handle line boundaries
        let mut remaining = content;

        while !remaining.is_empty() {
            let (action, update, consumed) = self.process_chunk(remaining, stream);
            if let Some(action) = action {
                actions.push(action);
            }
            if update.is_some() {
                debug_update = update;
            }
            remaining = &remaining[consumed..];
        }

        (actions, debug_update)
    }

    /// Process a chunk of content, returning (action, debug_update, bytes_consumed)
    fn process_chunk(
        &mut self,
        content: &str,
        stream: Stream,
    ) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>, usize) {
        match &mut self.state {
            ConsoleFilterState::Passthrough { at_line_start } => {
                if *at_line_start {
                    // At line boundary, check if content could match a prefix
                    match try_match_prefix(content) {
                        PrefixMatch::Full(pattern) => {
                            let prefix_len = pattern.prefix().len();
                            self.state = ConsoleFilterState::Filtering {
                                pattern,
                                expr_buffer: String::new(),
                                timestamp: Instant::now(),
                            };
                            (None, None, prefix_len)
                        },
                        PrefixMatch::Partial => {
                            // Start buffering
                            self.state = ConsoleFilterState::Buffering {
                                buffer: content.to_string(),
                                stream,
                                timestamp: Instant::now(),
                            };
                            (None, None, content.len())
                        },
                        PrefixMatch::None => {
                            // Emit content up to next newline
                            self.emit_until_newline(content, stream)
                        },
                    }
                } else {
                    // Not at line boundary, emit until we hit a newline
                    self.emit_until_newline(content, stream)
                }
            },

            ConsoleFilterState::Buffering {
                buffer,
                stream: buffered_stream,
                timestamp,
            } => {
                // Check timeout
                if timestamp.elapsed() > self.timeout {
                    let emit = buffer.clone();
                    let s = *buffered_stream;
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: emit.ends_with('\n'),
                    };
                    return (Some((emit, s)), None, 0);
                }

                // Append to buffer
                buffer.push_str(content);

                match try_match_prefix(buffer) {
                    PrefixMatch::Full(pattern) => {
                        // Full match! Extract any content after the prefix
                        let prefix_len = pattern.prefix().len();
                        let after_prefix = buffer[prefix_len..].to_string();
                        self.state = ConsoleFilterState::Filtering {
                            pattern,
                            expr_buffer: after_prefix,
                            timestamp: Instant::now(),
                        };
                        (None, None, content.len())
                    },
                    PrefixMatch::Partial => {
                        // Still partial, keep buffering
                        (None, None, content.len())
                    },
                    PrefixMatch::None => {
                        // Cannot match, flush buffer
                        let emit = buffer.clone();
                        let s = *buffered_stream;
                        self.state = ConsoleFilterState::Passthrough {
                            at_line_start: emit.ends_with('\n'),
                        };
                        (Some((emit, s)), None, content.len())
                    },
                }
            },

            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                timestamp,
            } => {
                // Check timeout: not confirmed as debug output, emit back to user
                if timestamp.elapsed() > self.timeout {
                    let text = format!("{}{}", pattern.prefix(), expr_buffer);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: expr_buffer.ends_with('\n'),
                    };
                    return (Some((text, Stream::Stdout)), None, 0);
                }

                // Append content to expression buffer
                expr_buffer.push_str(content);

                // Check if expression is complete
                let expr_text = extract_expression(*pattern, expr_buffer);
                match check_expression_complete(&expr_text) {
                    ExpressionStatus::Complete => {
                        let update = finalize_capture(*pattern, expr_buffer);
                        // After a complete expression, we're at a line start
                        // (the expression includes a trailing newline)
                        self.state = ConsoleFilterState::Passthrough {
                            at_line_start: true,
                        };
                        (None, Some(update), content.len())
                    },
                    ExpressionStatus::Incomplete => {
                        // Keep accumulating
                        (None, None, content.len())
                    },
                    ExpressionStatus::SyntaxError => {
                        // Not valid R, probably not real debug output. Emit back to user.
                        let text = format!("{}{}", pattern.prefix(), expr_buffer);
                        self.state = ConsoleFilterState::Passthrough {
                            at_line_start: expr_buffer.ends_with('\n'),
                        };
                        (Some((text, Stream::Stdout)), None, content.len())
                    },
                }
            },
        }
    }

    /// Emit content up to and including the next newline, updating state
    fn emit_until_newline(
        &mut self,
        content: &str,
        stream: Stream,
    ) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>, usize) {
        if let Some(newline_pos) = content.find('\n') {
            // Emit up to and including newline
            let (before, _) = content.split_at(newline_pos + 1);
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: true,
            };
            (Some((before.to_string(), stream)), None, newline_pos + 1)
        } else {
            // No newline, emit everything
            self.state = ConsoleFilterState::Passthrough {
                at_line_start: false,
            };
            (Some((content.to_string(), stream)), None, content.len())
        }
    }

    /// Called when ReadConsole is entered, flush/finalize any pending state.
    /// Reaching ReadConsole confirms that filtered content was real debug
    /// output (the expected sequence is debug message then browser prompt).
    /// Content in `Filtering` state is suppressed and finalized as a debug
    /// state update. Content in `Buffering` state (unconfirmed prefix match)
    /// is emitted back to the user.
    pub fn on_read_console(&mut self) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>) {
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => (None, None),
            ConsoleFilterState::Buffering { buffer, stream, .. } => (Some((buffer, stream)), None),
            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                ..
            } => (None, Some(finalize_capture(pattern, &expr_buffer))),
        }
    }

    /// Check for timeout and handle state transitions.
    /// Timeout means we didn't reach ReadConsole to confirm debug output,
    /// so we emit the accumulated content back to the user.
    pub fn check_timeout(&mut self) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>) {
        self.check_timeout_internal()
    }

    fn check_timeout_internal(
        &mut self,
    ) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>) {
        match &self.state {
            ConsoleFilterState::Passthrough { .. } => (None, None),
            ConsoleFilterState::Buffering {
                buffer,
                stream,
                timestamp,
            } => {
                if timestamp.elapsed() > self.timeout {
                    let emit = (buffer.clone(), *stream);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: buffer.ends_with('\n'),
                    };
                    (Some(emit), None)
                } else {
                    (None, None)
                }
            },
            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                timestamp,
            } => {
                if timestamp.elapsed() > self.timeout {
                    let text = format!("{}{}", pattern.prefix(), expr_buffer);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: expr_buffer.ends_with('\n'),
                    };
                    (Some((text, Stream::Stdout)), None)
                } else {
                    (None, None)
                }
            },
        }
    }

    /// Get any buffered content that should be emitted (for cleanup)
    pub fn flush(&mut self) -> Option<(String, Stream)> {
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => None,
            ConsoleFilterState::Buffering { buffer, stream, .. } => Some((buffer, stream)),
            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                ..
            } => {
                let text = format!("{}{}", pattern.prefix(), expr_buffer);
                Some((text, Stream::Stdout))
            },
        }
    }
}

/// Try to match content against known prefixes
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

/// Extract the expression portion from the buffer based on pattern type
fn extract_expression(pattern: MatchedPattern, buffer: &str) -> String {
    match pattern {
        MatchedPattern::DebugAt => {
            // Format: <path>#<line>: <expression>
            // Find the `#<digits>: ` pattern to locate expression start
            if let Some(expr_start) = find_debug_at_expression_start(buffer) {
                buffer[expr_start..].to_string()
            } else {
                // Haven't seen the `: ` after line number yet, return empty
                String::new()
            }
        },
        _ => {
            // For other patterns, expression starts immediately
            buffer.to_string()
        },
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

/// Status of expression parsing
enum ExpressionStatus {
    Complete,
    Incomplete,
    SyntaxError,
}

/// Check if an expression is syntactically complete using R's parser
fn check_expression_complete(expr: &str) -> ExpressionStatus {
    // Empty expression is incomplete
    if expr.trim().is_empty() {
        return ExpressionStatus::Incomplete;
    }

    // Use harp's parse_status to check expression completeness
    match parse_status(&ParseInput::Text(expr)) {
        Ok(ParseResult::Complete(_)) => ExpressionStatus::Complete,
        Ok(ParseResult::Incomplete) => ExpressionStatus::Incomplete,
        Ok(ParseResult::SyntaxError { .. }) => ExpressionStatus::SyntaxError,
        Err(_) => ExpressionStatus::SyntaxError,
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

/// Finalize the capture and produce the appropriate debug state update
fn finalize_capture(pattern: MatchedPattern, expr_buffer: &str) -> DebugCallTextUpdate {
    match pattern {
        MatchedPattern::Debug => {
            DebugCallTextUpdate::Finalized(expr_buffer.to_string(), DebugCallTextKind::Debug)
        },
        MatchedPattern::DebugAt => {
            // For DebugAt, extract just the expression part (after `path#line: `)
            // This is what maybe_auto_step expects when checking for auto-step expressions
            let expr = extract_expression(pattern, expr_buffer);
            DebugCallTextUpdate::Finalized(expr, DebugCallTextKind::DebugAt)
        },
        MatchedPattern::CalledFrom => {
            // CalledFrom is filtered but doesn't affect auto-stepping
            // We could track it separately if needed, but for now just suppress
            DebugCallTextUpdate::Finalized(expr_buffer.to_string(), DebugCallTextKind::Debug)
        },
        MatchedPattern::DebuggingIn | MatchedPattern::ExitingFrom => {
            // Reset debug state
            DebugCallTextUpdate::Reset
        },
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
    fn test_stderr_passthrough() {
        let mut filter = ConsoleFilter::new();
        let (actions, update) = filter.feed("error message\n", Stream::Stderr);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].0, "error message\n");
        assert_eq!(actions[0].1, Stream::Stderr);
        assert!(update.is_none());
    }

    #[test]
    fn test_normal_output_passthrough() {
        let mut filter = ConsoleFilter::new();
        let (actions, update) = filter.feed("Hello, world!\n", Stream::Stdout);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].0, "Hello, world!\n");
        assert_eq!(actions[0].1, Stream::Stdout);
        assert!(update.is_none());
    }

    #[test]
    fn test_buffering_state_on_partial_match() {
        let mut filter = ConsoleFilter::new();

        // Start with partial match - should buffer
        let (actions, _) = filter.feed("Called ", Stream::Stdout);
        // While buffering, nothing emitted yet
        assert!(actions.is_empty() || actions.iter().all(|(s, _)| s.is_empty()));
    }

    #[test]
    fn test_non_matching_prefix_emitted() {
        let mut filter = ConsoleFilter::new();

        // Content that starts like a prefix but doesn't match
        let (actions, _) = filter.feed("Calling function...\n", Stream::Stdout);

        // "Calling" doesn't match any prefix, should be emitted
        let emitted: String = actions
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
        let _ = filter.feed("Called ", Stream::Stdout);

        // Buffering is an unconfirmed prefix match, so ReadConsole emits it
        let (emit, update) = filter.on_read_console();

        let (text, stream) = emit.unwrap();
        assert_eq!(text, "Called ");
        assert_eq!(stream, Stream::Stdout);
        assert!(update.is_none());

        // After on_read_console, filter should be in Passthrough state
        let (actions, _) = filter.feed("Hello\n", Stream::Stdout);
        let emitted: String = actions
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
        let _ = filter.feed("Called ", Stream::Stdout);

        // Flush should return the buffered content
        let flushed = filter.flush();

        let (text, stream) = flushed.unwrap();
        assert_eq!(text, "Called ");
        assert_eq!(stream, Stream::Stdout);
    }
}
