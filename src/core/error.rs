//! 错误类型：统一全 crate 的 Result<T>。

use std::fmt;

#[derive(Debug)]
pub enum CurSeenError {
    Config(String),
    Io(std::io::Error),
    /// 带**路径 + 操作名**的 I/O 错误。
    ///
    /// 为什么要有它（总纲 §9.2）：`From<std::io::Error>` 那种自动转换会把"从哪来"
    /// 抹掉——同一个 `io::Error` 从读路径和写路径冒出来，日志里都只剩
    /// `io error: ...`，事后完全无法定位。凡**用户数据**的读写（配置、脚本、素材）
    /// 一律用这个变体，把路径与操作名留在日志里。
    IoAt {
        path: std::path::PathBuf,
        op: &'static str,
        source: std::io::Error,
    },
    Hook(String),
    Audio(String),
    Network(String),
    Update(String),
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, CurSeenError>;

impl CurSeenError {
    /// 构造 [`CurSeenError::IoAt`]。I/O 错误的**推荐**写法：带上路径与操作名。
    pub fn io_at<P: AsRef<std::path::Path>>(
        path: P,
        op: &'static str,
        source: std::io::Error,
    ) -> Self {
        Self::IoAt {
            path: path.as_ref().to_path_buf(),
            op,
            source,
        }
    }
}

impl fmt::Display for CurSeenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(s) => write!(f, "config error: {s}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::IoAt { path, op, source } => {
                write!(f, "io error: {op} {} 失败: {source}", path.display())
            }
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
            Self::IoAt { source, .. } => Some(source),
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
