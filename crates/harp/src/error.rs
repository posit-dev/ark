//
// error.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::backtrace::Backtrace;
use std::fmt;
use std::str::Utf8Error;

use crate::utils::r_type2char;

pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    HelpTopicNotFoundError {
        topic: String,
        package: Option<String>,
    },
    ParseError {
        code: String,
        message: String,
    },
    TryCatchError {
        code: Option<String>,
        message: String,
        class: Option<Vec<String>>,
        r_trace: String,
        rust_trace: Option<Backtrace>,
    },
    TopLevelExecError {
        message: String,
        backtrace: Backtrace,
        span_trace: tracing_error::SpanTrace,
    },
    UnsafeEvaluationError(String),
    UnexpectedLength(usize, usize),
    UnexpectedType(u32, Vec<u32>),
    ValueOutOfRange {
        value: i64,
        min: i64,
        max: i64,
    },
    InvalidUtf8(Utf8Error),
    ParseSyntaxError {
        message: String,
        line: i32,
    },
    MissingValueError,
    MissingBindingError {
        name: String,
    },
    OutOfMemory {
        size: usize,
    },
    InspectError {
        path: Vec<String>,
    },
    StackUsageError {
        message: String,
        backtrace: Backtrace,
        span_trace: tracing_error::SpanTrace,
    },
    Anyhow(anyhow::Error),
}

pub const R_BACKTRACE_HEADER: &str = "R thread backtrace:";

// empty implementation required for 'anyhow'
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InvalidUtf8(source) => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::HelpTopicNotFoundError { topic, package } => match package {
                Some(package) => write!(
                    f,
                    "Help topic '{}' not available in package '{}'",
                    topic, package
                ),
                None => write!(f, "Help topic '{}' not available", topic),
            },

            Error::ParseError { code, message } => {
                write!(f, "Error parsing {}: {}", code, message)
            },

            Error::TryCatchError {
                code,
                message,
                r_trace,
                rust_trace,
                ..
            } => {
                if let Some(code) = code {
                    let code = truncate_lines(code.clone(), 10);
                    write!(f, "Error evaluating '{code}': {message}")?;
                } else {
                    write!(f, "{message}")?;
                }

                // We display intercepting backtraces in Display instead of
                // Debug because anyhow doesn't propagate the `?` flag:
                // https://users.rust-lang.org/t/why-doesnt-anyhows-debug-formatter-include-the-underlying-debug-formatting/44227

                if !r_trace.is_empty() {
                    let r_trace = truncate_lines(r_trace.clone(), 500);
                    writeln!(f, "\n\nR backtrace:\n{r_trace}")?;
                }

                if let Some(rust_trace) = rust_trace {
                    writeln!(f, "\n\n{R_BACKTRACE_HEADER}\n{rust_trace}")?;
                }

                Ok(())
            },

            Error::TopLevelExecError {
                message,
                span_trace,
                backtrace,
                ..
            } => {
                writeln!(f, "{message}")?;

                if span_trace.status() == tracing_error::SpanTraceStatus::CAPTURED {
                    writeln!(f, "\n\nIn spans:")?;
                    span_trace.fmt(f)?;
                }

                writeln!(f, "\n\n{R_BACKTRACE_HEADER}\n{backtrace}")?;
                fmt::Display::fmt(backtrace, f)?;

                Ok(())
            },

            Error::UnsafeEvaluationError(code) => {
                write!(
                    f,
                    "Evaluation of function calls not supported in this context: {}",
                    code
                )
            },

            Error::UnexpectedLength(actual, expected) => {
                write!(
                    f,
                    "Unexpected vector length (expected {}; got {})",
                    expected, actual
                )
            },

            Error::UnexpectedType(actual, expected) => {
                let actual = r_type2char(*actual);
                let expected = expected
                    .iter()
                    .map(|value| r_type2char(*value))
                    .collect::<Vec<_>>()
                    .join(" | ");
                write!(
                    f,
                    "Unexpected vector type (expected {}; got {})",
                    expected, actual
                )
            },

            Error::ValueOutOfRange { value, min, max } => {
                write!(
                    f,
                    "Value is out of range: value: {} min: {} max: {}",
                    value, min, max
                )
            },

            Error::InvalidUtf8(error) => {
                write!(f, "Invalid UTF-8 in string: {}", error)
            },

            Error::ParseSyntaxError { message, line } => {
                write!(f, "Syntax error on line {} when parsing: {}", line, message)
            },

            Error::MissingValueError => {
                write!(f, "Missing value")
            },

            Error::InspectError { path } => {
                write!(f, "Error inspecting path {}", path.join(" / "))
            },

            Error::StackUsageError { .. } => {
                write!(f, "C stack usage too close to the limit")
            },

            Error::Anyhow(err) => {
                write!(f, "{err:?}")
            },

            Error::MissingBindingError { name } => {
                write!(f, "Can't find binding {name} in environment")
            },

            Error::OutOfMemory { size } => {
                write!(
                    f,
                    "Can't allocate object of size {size} as the system is out of memory"
                )
            },
        }
    }
}

#[macro_export]
macro_rules! anyhow {
    ($($rest: expr),*) => {{
        let message = anyhow::anyhow!($($rest, )*);
        crate::error::Error::Anyhow(message)
    }}
}

pub fn as_result<T, E>(res: std::result::Result<T, E>) -> crate::Result<T>
where
    E: std::fmt::Debug,
{
    match res {
        Ok(x) => Ok(x),
        Err(err) => Err(crate::anyhow!("{err:?}")),
    }
}

// We include R-level backtraces in `Display` because anyhow doesn't propagate the `?` flag:
// https://users.rust-lang.org/t/why-doesnt-anyhows-debug-formatter-include-the-underlying-debug-formatting/44227
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// TODO: Macro variants of `check_` helpers that record function name, see
// `function_name` in https://docs.rs/stdext/latest/src/stdext/macros.rs.html

fn check(x: impl Into<libr::SEXP>, expected: libr::SEXPTYPE) -> crate::Result<()> {
    let x = x.into();
    let typ = crate::r_typeof(x);

    if typ != expected {
        let err = Error::UnexpectedType(typ, vec![expected]);
        return Err(err);
    }

    Ok(())
}

pub fn check_env(x: impl Into<libr::SEXP>) -> crate::Result<()> {
    check(x, libr::ENVSXP)
}

impl From<Utf8Error> for Error {
    fn from(error: Utf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}

fn truncate_lines(text: String, max_lines: usize) -> String {
    let n_lines = text.lines().count();
    if n_lines <= max_lines {
        return text;
    }

    let mut text: String = text.lines().take(max_lines).collect();
    text.push_str(&format!("... *Truncated {} lines*", n_lines - max_lines));

    text
}
