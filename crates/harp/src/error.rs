//
// error.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
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
    EvaluationError {
        code: String,
        message: String,
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
    TryCatchError {
        message: Vec<String>,
        classes: Vec<String>,
    },
    TryEvalError {
        message: String,
    },
    TopLevelExecError {
        message: String,
        backtrace: Backtrace,
    },
    ParseSyntaxError {
        message: String,
        line: i32,
    },
    MissingValueError,
    InspectError {
        path: Vec<String>,
    },
    StackUsageError {
        message: String,
        backtrace: Backtrace,
    },
}

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

            Error::EvaluationError { code, message } => {
                write!(f, "Error evaluating {}: {}", code, message)
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

            Error::UnexpectedType(actual, expected) => unsafe {
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

            Error::TryCatchError {
                message,
                classes: _,
            } => {
                let message = message.join("\n");
                write!(f, "tryCatch error: {message}")
            },

            Error::TryEvalError { message } => {
                write!(f, "`eval()` error: {}", message)
            },

            Error::TopLevelExecError {
                message,
                backtrace: _,
            } => {
                write!(f, "`R_topLevelExec()` error: {}", message)
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
        }
    }
}

// NOTE: Debug is the same as Display but with backtrace printing.
// This matches anyhow error formatters and we can still retrieve the
// struct-style format with `{:#?}`.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)?;

        match self {
            Error::TopLevelExecError {
                message: _,
                backtrace,
            } => fmt::Display::fmt(backtrace, f),
            _ => Ok(()),
        }
    }
}

impl From<Utf8Error> for Error {
    fn from(error: Utf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}
