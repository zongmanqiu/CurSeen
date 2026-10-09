//! 错误类型：统一全 crate 的 Result<T>。

use std::fmt;

#[derive(Debug)]
pub enum CurSeenError {
    Config(String),
    Io(std::io::Error),
    Hook(String),
    Audio(String),
    Network(String),
    Update(String),
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, CurSeenError>;

impl fmt::Display for CurSeenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(s) => write!(f, "config error: {s}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Hook(s) => write!(f, "hook error: {s}"),
            Self::Audio(s) => write!(f, "audio error: {s}"),
            Self::Network(s) => write!(f, "network error: {s}"),
            Self::Update(s) => write!(f, "update error: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for CurSeenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CurSeenError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for CurSeenError {
    fn from(e: serde_json::Error) -> Self {
        Self::Config(e.to_string())
    }
}
