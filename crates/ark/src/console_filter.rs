//
// console_filter.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
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

    /// Whether the text after the prefix is an R expression that should
    /// be validated with the parser. `Debug` and `DebugAt` contain actual
    /// R code; the others contain context descriptions like `"top level"`
    /// and are completed on newline instead.
    fn has_r_expression(&self) -> bool {
        matches!(self, MatchedPattern::Debug | MatchedPattern::DebugAt)
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

    /// Confirmed prefix match. Accumulating the expression that follows.
    /// We parse the expression to determine when it's complete.
    Filtering {
        pattern: MatchedPattern,
        expr_buffer: String,
        timestamp: Instant,
        /// Whether the console was in a debug session when this match started.
        was_debugging: bool,
    },
}

/// A captured debug message awaiting confirmation at ReadConsole
struct PendingCapture {
    pattern: MatchedPattern,
    expr_buffer: String,
    was_debugging: bool,
    timestamp: Instant,
}

/// Filter for debug console output
pub struct ConsoleFilter {
    state: ConsoleFilterState,
    /// Captured debug messages awaiting ReadConsole confirmation
    pending: Vec<PendingCapture>,
    timeout: Duration,
    /// Whether we're currently inside a debug session. Updated by the
    /// console so the filter can record context when entering `Filtering`.
    is_debugging: bool,
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
            pending: Vec::new(),
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
            pending: Vec::new(),
            timeout,
            is_debugging: false,
        }
    }

    pub fn set_debugging(&mut self, is_debugging: bool) {
        self.is_debugging = is_debugging;
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

        let mut actions: Vec<(String, Stream)> = Vec::new();
        let mut debug_update: Option<DebugCallTextUpdate> = None;

        // Check for timed-out pending captures first
        let (pending_emits, pending_update) = self.check_pending_timeouts();
        actions.extend(pending_emits);
        if pending_update.is_some() {
            debug_update = pending_update;
        }

        // Check current state timeout
        let (timeout_emit, timeout_update) = self.check_state_timeout();
        if let Some(emit) = timeout_emit {
            actions.push(emit);
        }
        if timeout_update.is_some() {
            debug_update = timeout_update;
        }

        // Process content chunk by chunk to handle line boundaries
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
                                was_debugging: self.is_debugging,
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
                            was_debugging: self.is_debugging,
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
                was_debugging,
            } => {
                // Check timeout
                if timestamp.elapsed() > self.timeout {
                    let text = format!("{}{}", pattern.prefix(), expr_buffer);
                    self.state = ConsoleFilterState::Passthrough {
                        at_line_start: expr_buffer.ends_with('\n'),
                    };
                    return (Some((text, Stream::Stdout)), None, 0);
                }

                if pattern.has_r_expression() {
                    // `Debug` and `DebugAt`: use R's parser to decide
                    // when the expression is complete.
                    expr_buffer.push_str(content);
                    let expr_to_parse = extract_expression(*pattern, expr_buffer);

                    match check_expression_status(&expr_to_parse) {
                        ExpressionStatus::Complete => {
                            self.pending.push(PendingCapture {
                                pattern: *pattern,
                                expr_buffer: expr_buffer.clone(),
                                was_debugging: *was_debugging,
                                timestamp: *timestamp,
                            });
                            self.state = ConsoleFilterState::Passthrough {
                                at_line_start: expr_buffer.ends_with('\n'),
                            };
                            (None, None, content.len())
                        },
                        ExpressionStatus::Incomplete => (None, None, content.len()),
                        ExpressionStatus::SyntaxError => {
                            // Not a valid R expression — not a real debug
                            // message. Flush-emit everything.
                            let text = format!("{}{}", pattern.prefix(), expr_buffer);
                            self.state = ConsoleFilterState::Passthrough {
                                at_line_start: expr_buffer.ends_with('\n'),
                            };
                            (Some((text, Stream::Stdout)), None, content.len())
                        },
                    }
                } else {
                    // `CalledFrom`, `DebuggingIn`, `ExitingFrom`: the text
                    // after the prefix is a context description (e.g.,
                    // "top level") or a function call that may span multiple
                    // lines. Use the parser to detect completion, but treat
                    // SyntaxError as complete since these are always real
                    // debug messages once the prefix has matched. Only
                    // append up to the first newline so that subsequent
                    // unrelated content is not swallowed into the capture.
                    let (to_append, consumed) = match content.find('\n') {
                        Some(pos) => (&content[..=pos], pos + 1),
                        None => (content, content.len()),
                    };
                    expr_buffer.push_str(to_append);

                    match check_expression_status(expr_buffer) {
                        ExpressionStatus::Complete | ExpressionStatus::SyntaxError => {
                            self.pending.push(PendingCapture {
                                pattern: *pattern,
                                expr_buffer: expr_buffer.clone(),
                                was_debugging: *was_debugging,
                                timestamp: *timestamp,
                            });
                            self.state = ConsoleFilterState::Passthrough {
                                at_line_start: expr_buffer.ends_with('\n'),
                            };
                            (None, None, consumed)
                        },
                        ExpressionStatus::Incomplete => (None, None, consumed),
                    }
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

        // Process all pending captures
        for capture in self.pending.drain(..) {
            if capture.was_debugging || is_browser {
                // Suppress and produce debug state update
                debug_update = Some(finalize_capture(capture.pattern, &capture.expr_buffer));
            } else {
                // Emit back to user
                let text = format!("{}{}", capture.pattern.prefix(), capture.expr_buffer);
                emits.push((text, Stream::Stdout));
            }
        }

        // Process current state
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => {},
            ConsoleFilterState::Buffering { buffer, stream, .. } => {
                emits.push((buffer, stream));
            },
            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                was_debugging,
                ..
            } => {
                if was_debugging || is_browser {
                    debug_update = Some(finalize_capture(pattern, &expr_buffer));
                } else {
                    let text = format!("{}{}", pattern.prefix(), expr_buffer);
                    emits.push((text, Stream::Stdout));
                }
            },
        }

        let emits_opt = if emits.is_empty() { vec![] } else { emits };
        (emits_opt, debug_update)
    }

    /// Check for timeout and handle state transitions.
    /// Timeout means we didn't reach ReadConsole to confirm debug output,
    /// so we emit the accumulated content back to the user.
    pub fn check_timeout(&mut self) -> (Vec<(String, Stream)>, Option<DebugCallTextUpdate>) {
        let mut emits: Vec<(String, Stream)> = Vec::new();
        let mut debug_update: Option<DebugCallTextUpdate> = None;

        let (pending_emits, pending_update) = self.check_pending_timeouts();
        emits.extend(pending_emits);
        if pending_update.is_some() {
            debug_update = pending_update;
        }

        let (state_emit, state_update) = self.check_state_timeout();
        if let Some(emit) = state_emit {
            emits.push(emit);
        }
        if state_update.is_some() {
            debug_update = state_update;
        }

        (emits, debug_update)
    }

    /// Check pending captures for timeouts
    fn check_pending_timeouts(&mut self) -> (Vec<(String, Stream)>, Option<DebugCallTextUpdate>) {
        let mut emits: Vec<(String, Stream)> = Vec::new();
        let mut timed_out_indices: Vec<usize> = Vec::new();

        for (i, capture) in self.pending.iter().enumerate() {
            if capture.timestamp.elapsed() > self.timeout {
                let text = format!("{}{}", capture.pattern.prefix(), capture.expr_buffer);
                emits.push((text, Stream::Stdout));
                timed_out_indices.push(i);
            }
        }

        // Remove timed out captures in reverse order to preserve indices
        for i in timed_out_indices.into_iter().rev() {
            self.pending.remove(i);
        }

        (emits, None)
    }

    /// Check current state for timeout
    fn check_state_timeout(&mut self) -> (Option<(String, Stream)>, Option<DebugCallTextUpdate>) {
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
                ..
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
    pub fn flush(&mut self) -> Vec<(String, Stream)> {
        let mut emits: Vec<(String, Stream)> = Vec::new();

        // Flush pending captures
        for capture in self.pending.drain(..) {
            let text = format!("{}{}", capture.pattern.prefix(), capture.expr_buffer);
            emits.push((text, Stream::Stdout));
        }

        // Flush current state
        match std::mem::replace(&mut self.state, ConsoleFilterState::Passthrough {
            at_line_start: true,
        }) {
            ConsoleFilterState::Passthrough { .. } => {},
            ConsoleFilterState::Buffering { buffer, stream, .. } => {
                emits.push((buffer, stream));
            },
            ConsoleFilterState::Filtering {
                pattern,
                expr_buffer,
                ..
            } => {
                let text = format!("{}{}", pattern.prefix(), expr_buffer);
                emits.push((text, Stream::Stdout));
            },
        }

        emits
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
    /// Expression is syntactically complete
    Complete,
    /// Expression is syntactically valid but incomplete (e.g., unclosed brace)
    Incomplete,
    /// Expression has a syntax error
    SyntaxError,
}

/// Check whether an expression string is complete, incomplete, or has a syntax error
fn check_expression_status(expr: &str) -> ExpressionStatus {
    // Empty or whitespace-only is incomplete
    if expr.trim().is_empty() {
        return ExpressionStatus::Incomplete;
    }

    // Use R's parser to check the expression status
    match parse_status(&ParseInput::Text(expr)) {
        Ok(ParseResult::Complete(_)) => ExpressionStatus::Complete,
        Ok(ParseResult::Incomplete) => ExpressionStatus::Incomplete,
        Ok(ParseResult::SyntaxError { .. }) => ExpressionStatus::SyntaxError,
        Err(_) => {
            // Parser error - treat as syntax error to be safe
            ExpressionStatus::SyntaxError
        },
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

/// Strip debug prefix lines from a string, used to clean `autoprint_output`
/// at browser prompts.
///
/// For `debug at` and `debug:` patterns the prefix is stripped but the
/// expression rendering is kept (it is what the user sees as the step
/// result). For other patterns (`Called from:`, `debugging in:`,
/// `exiting from:`) the entire line is removed.
pub fn strip_debug_prefix_lines(text: &mut String) {
    if text.is_empty() {
        return;
    }
    let mut result = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.starts_with(MatchedPattern::DebugAt.prefix()) {
            let after_prefix = &line[MatchedPattern::DebugAt.prefix().len()..];
            if let Some(expr_start) = find_debug_at_expression_start(after_prefix) {
                result.push_str(&after_prefix[expr_start..]);
            }
        } else if line.starts_with(MatchedPattern::Debug.prefix()) {
            result.push_str(&line[MatchedPattern::Debug.prefix().len()..]);
        } else if MatchedPattern::all()
            .iter()
            .any(|p| line.starts_with(p.prefix()))
        {
            // `Called from:`, `debugging in:`, `exiting from:` → drop entirely
        } else {
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
        // regardless of whether it's a browser prompt or not
        let (emits, update) = filter.on_read_console(false);

        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "Called ");
        assert_eq!(*stream, Stream::Stdout);
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
        // Prefix matched but no expression content follows and no
        // ReadConsole arrives. Timeout emits the prefix back to the user.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));

        // Feed just the prefix — enters Filtering with empty expr_buffer,
        // which is Incomplete without needing R's parser.
        let (actions, _) = filter.feed("debug: ", Stream::Stdout);
        assert!(actions.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let (emits, update) = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "debug: ");
        assert_eq!(*stream, Stream::Stdout);
        assert!(update.is_none());
    }

    #[test]
    fn test_adversarial_buffering_timeout_emits() {
        // Partial prefix match that times out before resolving
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
        let (actions, _) = filter.feed("debug", Stream::Stdout);
        assert!(actions.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let (emits, _) = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, stream) = &emits[0];
        assert_eq!(text, "debug");
        assert_eq!(*stream, Stream::Stdout);
    }

    #[test]
    fn test_adversarial_all_prefixes_timeout_emit() {
        for prefix in MatchedPattern::all() {
            let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
            let (actions, _) = filter.feed(prefix.prefix(), Stream::Stdout);
            assert!(actions.is_empty());

            std::thread::sleep(Duration::from_millis(5));

            let (emits, update) = filter.check_timeout();
            assert_eq!(emits.len(), 1);
            let (text, _) = &emits[0];
            assert_eq!(text, prefix.prefix());
            assert!(update.is_none());
        }
    }

    #[test]
    fn test_adversarial_debug_at_malformed_path_timeout_emits() {
        // "debug at " without the expected `#<digits>: ` pattern means the
        // expression extractor never finds a start, so the expression stays
        // empty (Incomplete). Timeout recovers the content.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));

        // Feed prefix separately so it's consumed, then the malformed path
        // enters the expr_buffer but stays Incomplete (empty extracted expr).
        let (actions, _) = filter.feed("debug at ", Stream::Stdout);
        assert!(actions.is_empty());
        let (actions, _) = filter.feed("not-a-path\n", Stream::Stdout);
        assert!(actions.is_empty());

        std::thread::sleep(Duration::from_millis(5));

        let (emits, update) = filter.check_timeout();
        assert_eq!(emits.len(), 1);
        let (text, _) = &emits[0];
        assert_eq!(text, "debug at not-a-path\n");
        assert!(update.is_none());
    }

    #[test]
    fn test_adversarial_prefix_mid_line_passes_through() {
        // Prefix text that doesn't start at a line boundary is not filtered
        let mut filter = ConsoleFilter::new();
        let (actions, update) = filter.feed("foo debug: bar\n", Stream::Stdout);

        assert_eq!(collect_stdout(&actions), "foo debug: bar\n");
        assert!(update.is_none());
    }

    #[test]
    fn test_adversarial_partial_prefix_then_non_matching() {
        // "Cal" looks like start of "Called from: " but next chunk is "culator"
        let mut filter = ConsoleFilter::new();
        let (actions1, _) = filter.feed("Cal", Stream::Stdout);
        assert!(actions1.is_empty());

        let (actions2, _) = filter.feed("culator\n", Stream::Stdout);
        assert_eq!(collect_stdout(&actions2), "Calculator\n");
    }

    #[test]
    fn test_adversarial_flush_recovers_filtering_state() {
        let mut filter = ConsoleFilter::new();
        let (actions, _) = filter.feed("Called from: ", Stream::Stdout);
        assert!(actions.is_empty());

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
        let (actions, _) = filter.feed("Called from: ", Stream::Stdout);
        assert!(actions.is_empty());

        let (emits, update) = filter.on_read_console(true);
        assert!(emits.is_empty());
        assert!(update.is_some());
    }

    #[test]
    fn test_adversarial_on_read_console_toplevel_emits() {
        // Content in Filtering state IS emitted when a top-level prompt
        // arrives, because that means it was user output matching a prefix.
        let mut filter = ConsoleFilter::new();
        let (actions, _) = filter.feed("Called from: ", Stream::Stdout);
        assert!(actions.is_empty());

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
        let _ = filter.feed("debug: ", Stream::Stdout);

        std::thread::sleep(Duration::from_millis(5));

        // Next feed triggers timeout recovery then processes new content
        let (actions, _) = filter.feed("normal output\n", Stream::Stdout);
        let emitted = collect_stdout(&actions);
        assert!(emitted.contains("debug: "));
        assert!(emitted.contains("normal output\n"));
    }

    #[test]
    fn test_adversarial_timeout_during_feed_emits() {
        // Timeout fires inside `feed` (via check_timeout at the start)
        // when new content arrives after the deadline.
        let mut filter = ConsoleFilter::new_with_timeout(Duration::from_millis(1));
        let _ = filter.feed("exiting from: ", Stream::Stdout);

        std::thread::sleep(Duration::from_millis(5));

        let (actions, update) = filter.feed("next line\n", Stream::Stdout);
        let emitted = collect_stdout(&actions);
        assert!(emitted.contains("exiting from: "));
        assert!(emitted.contains("next line\n"));
        assert!(update.is_none());
    }
}
