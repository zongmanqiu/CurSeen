//! 分级日志，落盘到 `<程序目录>/log/curseen-YYYYMMDD.log`（便携化，见 `core::paths`）。
//!
//! 规则：
//!   · 不做单文件体积上限——按天分文件 + 保留 7 天（启动时清理）已经封住总量
//!   · 保留最近 7 天的日志（启动时清理）
//!   · 级别默认 Info；`CURSEEN_LOG_LEVEL=debug` 可临时开高频诊断（见 `init_logging`）
//!   · 写入走独立线程 + 无界队列（避免 UI 线程做磁盘 IO）
//!   · **捕获钩子回调内禁止调用日志**
//!
//! # 为什么落在程序目录、不进系统 temp
//!
//! 便携化的要求：整个程序就是"拷走一个目录"，删目录 = 卸载干净，日志与
//! `config.json` / `update/` 并列在 `data_dir()` 下（见 `core::paths`）。而系统临时目录
//! **会被磁盘清理 / 管家软件随时删掉**——日志唯一的价值是"出事后还在"，放那儿等于
//! 没有；崩溃报告（`platform::crash`）也写在同一目录，用户打包发给开发者时两者是配套的。

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

/// 日志目录：`<程序目录>/log/`（便携化，见 `core::paths`）。
fn log_dir() -> PathBuf {
    crate::core::paths::log_dir()
}

/// 当天日志文件名：`curseen-YYYYMMDD.log`
fn today_log_file() -> PathBuf {
    let now = chrono::Local::now();
    log_dir().join(format!("curseen-{}.log", now.format("%Y%m%d")))
}

/// 日志消息
enum LogMsg {
    Write(String),
    Flush,
    Shutdown,
}

/// 日志写入器
struct LogWriter {
    receiver: mpsc::Receiver<LogMsg>,
    writer: Option<BufWriter<fs::File>>,
}

impl LogWriter {
    fn new(receiver: mpsc::Receiver<LogMsg>) -> Self {
        Self {
            receiver,
            writer: None,
        }
    }

    fn ensure_writer(&mut self) {
        if self.writer.is_some() {
            return;
        }
        let path = today_log_file();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                self.writer = Some(BufWriter::new(f));
            }
            Err(e) => {
                eprintln!("[curseen] 无法打开日志文件: {}", e);
            }
        }
    }

    fn clean_old_logs(&self) {
        let dir = log_dir();
        if !dir.exists() {
            return;
        }
        let cutoff = chrono::Local::now() - chrono::Duration::days(7);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("log") {
                    if let Ok(meta) = fs::metadata(&path) {
                        if let Ok(modified) = meta.modified() {
                            if modified < cutoff.into() {
                                let _ = fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 写一行（不落盘，由排空逻辑统一 flush）。
    fn write_line(&mut self, line: &str) {
        self.ensure_writer();
        if let Some(writer) = self.writer.as_mut() {
            let _ = writeln!(writer, "{}", line);
        }
    }

    fn flush_now(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
        }
    }

    /// 写入线程主循环。
    ///
    /// ★ 落盘时机：**队列排空即 flush**，而不是靠定时器或进程退出。
    ///
    /// 之前只在缓冲满 8KB 或收到 `Shutdown` 时才落盘，而 `Logger` 挂在 `static` 上永不
    /// Drop（正常退出、被任务管理器结束、崩溃都不会发 `Shutdown`），结果是**日志文件
    /// 长期是 0 字节**——出问题想查日志时什么都没有。
    ///
    /// 排空即 flush 的代价只有一次 `flush` 系统调用，而且突发（一次连写 N 行）只 flush
    /// 一次；空闲时线程阻塞在 `recv()` 上，不引入任何定时唤醒（守住"空闲 0% CPU"）。
    fn run(&mut self) {
        self.clean_old_logs();
        while let Ok(msg) = self.receiver.recv() {
            match msg {
                LogMsg::Write(line) => {
                    self.write_line(&line);
                    // 把已排队的后续行一次写完，再统一落盘。
                    loop {
                        match self.receiver.try_recv() {
                            Ok(LogMsg::Write(next)) => self.write_line(&next),
                            Ok(LogMsg::Flush) => {}
                            Ok(LogMsg::Shutdown) => {
                                self.flush_now();
                                return;
                            }
                            Err(_) => break, // 队列已空（或发送端消失）
                        }
                    }
                    self.flush_now();
                }
                LogMsg::Flush => self.flush_now(),
                LogMsg::Shutdown => {
                    self.flush_now();
                    break;
                }
            }
        }
    }
}

/// 日志句柄
pub struct Logger {
    sender: mpsc::Sender<LogMsg>,
    _thread: JoinHandle<()>,
}

impl Logger {
    pub fn init() -> Self {
        let (tx, rx) = mpsc::channel();
        let mut writer = LogWriter::new(rx);
        let handle = thread::spawn(move || {
            writer.run();
        });
        Self {
            sender: tx,
            _thread: handle,
        }
    }

    pub fn log(&self, level: log::Level, target: &str, message: &str) {
        let now = chrono::Local::now();
        let line = format!(
            "[{}] [{:>5}] {}: {}",
            now.format("%H:%M:%S%.3f"),
            level,
            target,
            message
        );
        let _ = self.sender.send(LogMsg::Write(line));
    }

    pub fn flush(&self) {
        let _ = self.sender.send(LogMsg::Flush);
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        let _ = self.sender.send(LogMsg::Shutdown);
    }
}

/// 全局日志实例
static LOGGER: OnceLock<Logger> = OnceLock::new();

struct GlobalLogger;

impl log::Log for GlobalLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Some(logger) = LOGGER.get() {
            logger.log(record.level(), record.target(), &record.args().to_string());
        }
    }
    fn flush(&self) {
        if let Some(logger) = LOGGER.get() {
            logger.flush();
        }
    }
}

/// 初始化全局日志。应只在 main 中调用一次。
///
/// 级别：默认 **Info**；设环境变量 `CURSEEN_LOG_LEVEL=debug`（或 `trace` / `warn` /
/// `error`）可改。★ 走 debug 是**降噪**的落点——高频诊断（`debug!`）平时不落盘，
/// 排查时开一次即可，不必为了给日志瘦身而删掉它们。
pub fn init_logging() {
    let logger = Logger::init();
    let _ = LOGGER.set(logger);
    log::set_max_level(log_level_from_env());
    log::set_logger(&GlobalLogger).ok();
}

/// 从 `CURSEEN_LOG_LEVEL` 读级别；缺失或无法识别 → Info。
fn log_level_from_env() -> log::LevelFilter {
    let raw = std::env::var("CURSEEN_LOG_LEVEL").unwrap_or_default();
    parse_level(&raw)
}

/// 级别串 → `LevelFilter`（**不认识的一律 Info**，不会因为拼错就静默不记日志）。
fn parse_level(raw: &str) -> log::LevelFilter {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

/// 安装 panic 钩子：把 panic 消息与堆栈**同步**写进当天日志。
///
/// 存在的理由很直接：GUI 进程没有控制台，Rust 默认的 panic 输出会直接消失，用户只看
/// 到"程序没了"，日志里也一行不留——2026-09-15 的「切换蛛丝缠网崩溃」最初就是这样
/// 完全无迹可寻的（真凶是 `rand::gen_range` 的空区间 panic）。
///
/// 这里刻意**不走**异步日志通道：panic 之后进程可能立刻退出（或再次 panic），专用
/// 写线程多半来不及落盘。钩子直接开文件追加写明，代价只在崩溃这一次。
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg: &str = if let Some(s) = payload.downcast_ref::<&str>() {
            s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "（非字符串 panic 载荷）"
        };
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "（位置未知）".to_string());
        let now = chrono::Local::now();
        let line = format!(
            "[{}] [FATAL] panic: {msg}\n  at {loc}\n{}",
            now.format("%H:%M:%S%.3f"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(today_log_file())
        {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::parse_level;
    use log::LevelFilter;

    /// 拼错 / 留空一律退回 Info——"什么都记不到"比"多记一点"糟得多。
    #[test]
    fn unknown_log_level_falls_back_to_info() {
        assert_eq!(parse_level(""), LevelFilter::Info);
        assert_eq!(parse_level("  "), LevelFilter::Info);
        assert_eq!(parse_level("verbose"), LevelFilter::Info);
        assert_eq!(parse_level("Debug"), LevelFilter::Debug, "大小写与空白无关");
        assert_eq!(parse_level(" trace "), LevelFilter::Trace);
        assert_eq!(parse_level("off"), LevelFilter::Off);
    }
}
