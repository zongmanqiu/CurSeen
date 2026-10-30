//! 起外部进程（m10）——**全仓唯一**允许碰 `std::process::Command` 的模块。
//!
//! # 为什么要有这个"单点"
//!
//! 脚本能调外部程序 = 打开了本机执行任意代码的门。这条门的口径先说死：**它不是安全边界**
//! （`ext:write_file` 能把 exe 写进脚本目录再被起起来），信任模型是"装脚本 = 交出账号权限"。
//! 所以这里的"单点"买的是**可审计**与**行为可预期**，不是隔离。
//! 收口不写在调用点、写在**这一处**：
//! 谁在跑、跑多久、输出多大、超时了怎么收，全在这里定死；上层（`scripting::api_ext`）
//! 只负责"这个路径是不是脚本自己目录里的"这一条路径判据（见 `api_ext::resolve_tool`）。
//! 于是将来要审计"脚本到底能跑什么"，只需要读这一个文件加那一个目录——这条由
//! [`only_process_module_may_spawn`] 那条测试钉住（现存两处历史调用点在白名单里，见该测试）。
//!
//! # 三条硬纪律（每条都对应一个真实症状）
//!
//! 1. **绝不拼命令行**：`args` 是数组，逐个传给 `CreateProcessW` 的参数向量，
//!    所以脚本传 `{"--out", "a b.png"}` 不会被拆成两个参数，也没有 `cmd` 元字符可注入。
//! 2. **必须超时 + kill**：调用方是脚本自己的工作线程，而 `ext_runner` 的进程级去重表
//!    （`try_claim`）按脚本 id 占位——一个不退出的子进程会让"这个脚本热键按一次就废了"。
//!    所以超时一定 `kill`，且**任何**返回路径都要先收尸（`wait`），不留僵尸。
//! 3. **输出截字节**：`stdout`/`stderr` 各读满上限就停（[`MAX_OUTPUT`]），
//!    否则一个爱打日志的工具能把整份二进制吐进内存再弹一个框。

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `stdout` / `stderr` 各自最多读这么多字节（超出即截断，并置 `truncated`）。
pub const MAX_OUTPUT: usize = 1 << 20;
/// 默认超时（毫秒）。脚本可以传更小的值，但不能超过 [`MAX_TIMEOUT_MS`]。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// 超时上限：再长也必须有个头，免得一个脚本把那条工作线程永久占住。
pub const MAX_TIMEOUT_MS: u64 = 120_000;
/// 起子进程时**不建控制台窗口**（`CREATE_NO_WINDOW`）。
///
/// 脚本调的多是命令行工具（离线 OCR 就是一个控制台 exe）：默认起法在 GUI 进程里会先
/// 弹出一个黑窗口再消失，用户看到的就是"识别一次闪一下终端"。
///
/// ★ 它只影响**窗口**，不影响管道：stdout/stderr 仍然照常重定向与捕获（本文件所有
///   真起进程的测试都在这条标志下跑）。子进程自己的 `GetConsoleWindow()` 会返回一个
///   没有窗口的隐藏控制台（不是 NULL），这是微软给这条标志的既定语义。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 轮询子进程的间隔：10 ms 足够手感好，也不会把 CPU 打出噪声。
const POLL: Duration = Duration::from_millis(10);
/// 子进程退出后**最多**再等读线程多久把管道字节送回来（见 `run` 里的注释）。
const PUMP_GRACE: Duration = Duration::from_millis(2_000);

/// 一次外部程序调用。
pub struct RunSpec<'a> {
    /// 已经解析好的**绝对**路径（路径判据在调用方：见 `api_ext::resolve_tool`，那条只管"是不是
    /// 脚本自己目录里的 .exe"，不是安全边界）。
    pub exe: &'a Path,
    /// 参数数组（不含 argv[0]）。
    pub args: &'a [String],
    /// 工作目录：`None` = 用 exe 所在目录（工具常按相对路径找自己的模型/dll）。
    pub cwd: Option<&'a Path>,
    /// 超时（毫秒）。
    pub timeout_ms: u64,
    /// `true` = 按**系统 ANSI 码页**解 stdout/stderr（中文系统 = GBK）。
    ///
    /// ★ 为什么只能由调用方声明、宿主不许猜：两边都实测错过一次——
    ///   `RapidOcrOnnx.exe` 的 stdout **是 UTF-8**（按国标码解就吐"鏂囧瓧璇嗗埆"，
    ///   `examples/ext_proc_ocr.rs` 臂 1b 留了那格读数），而中文命令行工具里确有吐国标码的。
    ///   默认按 UTF-8（`from_utf8_lossy`），知道对面是系统码页的工具才把这一位置真。
    pub decode_ansi: bool,
}

/// 一次调用的结果。
pub struct RunOutput {
    /// 退出码；被我们 kill 掉时为 `None`。
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// 超时被杀。
    pub timed_out: bool,
    /// 输出触到 [`MAX_OUTPUT`] 上限（截过）。
    pub truncated: bool,
    /// 整次调用墙钟（毫秒）——**含起进程与加载 dll**，这才是用户感到的延迟。
    pub elapsed_ms: u64,
}

/// 起进程、等它结束（或超时杀掉）、收输出。
///
/// 不返回 `Result`：起不来（文件不存在、被杀软拦、权限不够）也算一次"有结论的调用"，
/// 用 [`RunOutput::stderr`] 带出去，脚本层据此如实提示，而不是抛一串 Rust 错误。
pub fn run(spec: RunSpec<'_>) -> RunOutput {
    let t0 = Instant::now();
    let timeout = Duration::from_millis(spec.timeout_ms.clamp(100, MAX_TIMEOUT_MS));
    let cwd = spec
        .cwd
        .unwrap_or_else(|| spec.exe.parent().unwrap_or(Path::new(".")));

    let mut child = match Command::new(spec.exe)
        .args(spec.args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 不弹控制台窗口（见 `CREATE_NO_WINDOW` 的文档）：脚本起 CLI 工具时不许闪黑框。
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return RunOutput {
                code: None,
                stdout: String::new(),
                stderr: format!("无法启动 {}：{e}", spec.exe.display()),
                timed_out: false,
                truncated: false,
                elapsed_ms: t0.elapsed().as_millis() as u64,
            }
        }
    };

    // ★ 两条管道必须**并发读**：管道缓冲区（约 40–64 KB）满了子进程就会写在阻塞，
    //   而我们在等它退出——两边互等就是死锁。所以各起一条线程读，读完把字节送回通道。
    let mut sout = child.stdout.take();
    let mut serr = child.stderr.take();
    let (tx1, rx1) = std::sync::mpsc::channel();
    let (tx2, rx2) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx1.send(drain(&mut sout));
    });
    std::thread::spawn(move || {
        let _ = tx2.send(drain(&mut serr));
    });

    let mut timed_out = false;
    let mut code: Option<i32> = None;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            // 退出码在这里就取走：`try_wait` 之后不能再靠 `child` 拿到它。
            Ok(Some(status)) => {
                code = status.code();
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    // `kill` 之后**必须** `wait` 收尸，否则进程表里留一个僵尸，
                    // 且 Windows 侧的句柄不会释放。
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(POLL);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = false;
                return RunOutput {
                    code: None,
                    stdout: String::new(),
                    stderr: format!("等待子进程时出错：{e}"),
                    timed_out,
                    truncated: false,
                    elapsed_ms: t0.elapsed().as_millis() as u64,
                };
            }
        }
    }

    // 子进程已经退了，管道本该随即 EOF；但**孙进程**可能还攥着同一个写句柄，
    // 那样读线程永远不会送回来。这里只等一小段（[`PUMP_GRACE`]），等不到就当输出不完整
    // 并置 `truncated`——这条函数**任何**情况都要在有限时间内返回，见模块头纪律 2。
    let (out, out_trunc) = pump(&rx1);
    let (err, err_trunc) = pump(&rx2);

    RunOutput {
        code: if timed_out { None } else { code },
        stdout: text(&out, spec.decode_ansi),
        stderr: if timed_out {
            format!("超时（{} ms）已终止进程", timeout.as_millis())
        } else {
            text(&err, spec.decode_ansi)
        },
        timed_out,
        truncated: out_trunc || err_trunc,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    }
}

/// 取一条读线程的成果，最多等 [`PUMP_GRACE`]。等不到 ⇒ 视作空输出 + 已截断。
fn pump(rx: &std::sync::mpsc::Receiver<(Vec<u8>, bool)>) -> (Vec<u8>, bool) {
    match rx.recv_timeout(PUMP_GRACE) {
        Ok(v) => v,
        Err(_) => (Vec::new(), true),
    }
}

/// 读满 [`MAX_OUTPUT`] 就停（多出来的字节直接丢掉，让子进程自己写进虚空）。
fn drain<R: Read>(pipe: &mut Option<R>) -> (Vec<u8>, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let Some(p) = pipe else {
        return (buf, false);
    };
    let mut chunk = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        match p.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = MAX_OUTPUT.saturating_sub(buf.len());
                let take = n.min(room);
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    truncated = true;
                    break; // 到上限：不再读，剩下的让它自己丢
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (buf, truncated)
}

/// 按声明解码子进程输出：`ansi=false` 走 UTF-8（坏字节换成替换字符），`true` 走系统码页。
///
/// ★ 判据来自实测（2026-10-28）：`RapidOcrOnnx.exe` 的 stdout 是 **UTF-8**，把它标成
///   `ansi` 就出来"鏂囧瓧璇嗗埆"（那格读数留在 `examples/ext_proc_ocr.rs` 的臂 1b）；
///   反过来，吐国标码的中文命令行工具按 UTF-8 读就是一串替换字符。
///   所以这条只由调用方声明，宿主不许替脚本猜。
fn text(bytes: &[u8], ansi: bool) -> String {
    if ansi {
        ansi_to_string(bytes)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// 系统 ANSI 码页（中文系统 = CP936/GBK）→ UTF-8。解不出来就退回 UTF-8 读法。
pub fn ansi_to_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    use windows::Win32::Globalization::{MultiByteToWideChar, CP_ACP, MB_ERR_INVALID_CHARS};
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, None);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; n as usize];
        let got = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, bytes, Some(&mut wide));
        if got <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        wide.truncate(got as usize);
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 把系统里一个**极小的** exe 复制进临时目录，当作"脚本目录里的外部程序"来验通路。
    ///
    /// 为什么不用 `cmd.exe`：`cmd` 是绝对路径的系统程序，而这条通路要验的恰恰是
    /// "只能跑脚本自己目录里的那个文件"。复制一份 `where.exe`（几十 KB）进临时目录，
    /// 既真又不污染仓库。
    fn copy_tiny_exe(dir: &Path, name: &str) -> Option<PathBuf> {
        let sys = PathBuf::from(std::env::var("WINDIR").ok()?)
            .join("System32")
            .join(name);
        if !sys.exists() {
            return None;
        }
        let dst = dir.join(name);
        std::fs::copy(&sys, &dst).ok()?;
        Some(dst)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "curseen_proc_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 真起一个进程、拿到退出码与输出——**通路本身**必须是活的读数，不是"代码看着对"。
    #[test]
    fn runs_a_real_process_and_captures_output() {
        let dir = temp_dir("run");
        let Some(exe) = copy_tiny_exe(&dir, "where.exe") else {
            eprintln!("找不到 where.exe，跳过");
            return;
        };
        let out = run(RunSpec {
            exe: &exe,
            // `where cmd` 把 cmd.exe 的完整路径打在 stdout 上（`where /?` 实测一个字节都不吐，
            // 所以不能拿它当"输出通路是活的"的证据）。
            args: &["cmd".to_string()],
            cwd: None,
            timeout_ms: 10_000,
            decode_ansi: false,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!out.timed_out, "不该超时：{}", out.stderr);
        assert_eq!(out.code, Some(0), "where cmd 应当正常退出：{}", out.stderr);
        assert!(
            out.stdout.to_lowercase().contains("cmd.exe"),
            "输出里应当有它找到的路径，实际前 80 字：{:?}",
            out.stdout.chars().take(80).collect::<String>()
        );
        assert!(!out.truncated, "几十个字节的输出不该判截断");
    }

    /// 超时必须**杀掉**子进程并返回，绝不让调用线程无限等下去。
    #[test]
    fn timeout_kills_and_returns() {
        let dir = temp_dir("timeout");
        let Some(exe) = copy_tiny_exe(&dir, "ping.exe") else {
            eprintln!("找不到 ping.exe，跳过");
            return;
        };
        let t0 = Instant::now();
        let out = run(RunSpec {
            exe: &exe,
            // 127.0.0.1 回包很快，但 -n 30 -w 1000 至少要 30 秒 → 必然撞上 1 秒超时
            args: &[
                "-n".to_string(),
                "30".to_string(),
                "-w".to_string(),
                "1000".to_string(),
                "127.0.0.1".to_string(),
            ],
            cwd: None,
            timeout_ms: 1_000,
            decode_ansi: false,
        });
        let took = t0.elapsed();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.timed_out, "应当判超时");
        assert_eq!(out.code, None, "被我们杀掉的进程不该报退出码");
        assert!(
            took < Duration::from_secs(6),
            "超时后必须马上返回，实际等了 {took:?}"
        );
    }

    /// 起不来的东西（不存在的文件）也要"有结论"，不许 panic、不许挂住。
    #[test]
    fn missing_exe_is_a_result_not_a_panic() {
        let dir = temp_dir("missing");
        let out = run(RunSpec {
            exe: &dir.join("nope.exe"),
            args: &[],
            cwd: None,
            timeout_ms: 2_000,
            decode_ansi: false,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.code.is_none());
        assert!(
            out.stderr.contains("无法启动"),
            "要能看懂为什么失败：{}",
            out.stderr
        );
    }

    /// ★ 码页解码必须是**活的读数**：吐国标码的中文命令行工具按 UTF-8 读会出一串替换字符，
    ///   而 `RapidOcrOnnx.exe` 反过来（UTF-8 的字节按国标码解）出"鏂囧瓧"——两头的症状都实测过。
    ///
    /// 样本是硬编码的国标码字节，所以这条只在**本机码页就是 936** 时立案——非中文系统上
    /// `CP_ACP` 不是国标码，那不是缺陷，如实打印跳过（不静默绿）。
    /// 同一串字节走 UTF-8 读法必然得到别的字，这条也一起断言：它证明真的经过了码页转换，
    /// 而不是函数顺手把输入原样吐回去。
    #[test]
    fn ansi_decodes_gbk_bytes_on_a_gbk_machine() {
        const GBK: &[u8] = &[
            0xCE, 0xC4, 0xD7, 0xD6, 0xCA, 0xB6, 0xB1, 0xF0, 0xB2, 0xE2, 0xCA, 0xD4,
        ]; // 「文字识别测试」的国标码
        const WANT: &str = "文字识别测试";
        assert_ne!(
            String::from_utf8_lossy(GBK).as_ref(),
            WANT,
            "样本必须**不是** UTF-8，否则这条测试什么都没测"
        );
        assert_eq!(ansi_to_string(&[]), "", "空输入不该报错也不该 panic");
        let acp = unsafe { windows::Win32::Globalization::GetACP() };
        if acp != 936 {
            eprintln!("本机 ANSI 码页是 {acp}（不是国标码 936），跳过这一格读数");
            return;
        }
        assert_eq!(ansi_to_string(GBK), WANT);
    }

    /// ★ 截字节与"读线程没回来"这两格必须有读数：它们各自挡的是一类真实故障
    ///   （爱打日志的工具把整份二进制吸进内存 / 孙进程攥着管道把脚本线程挂死）。
    #[test]
    fn output_cap_and_stalled_pipe_are_measured() {
        // 1) 截断：喂 2 倍上限的字节，只该留下 MAX_OUTPUT，并报告截过。
        let big: Vec<u8> = (0..2 * MAX_OUTPUT).map(|i| (i % 251) as u8).collect();
        let mut pipe = Some(std::io::Cursor::new(big.clone()));
        let (got, trunc) = drain(&mut pipe);
        assert_eq!(got.len(), MAX_OUTPUT, "必须停在上限，不多吸一个字节");
        assert!(trunc, "吸到上限要报告截过（脚本据此说\"输出太长已截断\"）");
        assert_eq!(&got[..16], &big[..16], "留下的必须是最前面那批字节");
        // 一个都没到上限的短输出不该误报截断。
        let mut small = Some(std::io::Cursor::new(vec![7u8; 32]));
        let (got, trunc) = drain(&mut small);
        assert_eq!(got.len(), 32);
        assert!(!trunc, "没到上限不许误报");

        // 2) 读线程回不来：通道被关掉（发送端随线程一起没了）⇒ `pump` 必须**立刻**
        //    给出"空输出 + 截断"，而不是等满 PUMP_GRACE 或永远挂着。
        let (tx, rx) = std::sync::mpsc::channel::<(Vec<u8>, bool)>();
        drop(tx);
        let t0 = Instant::now();
        let (bytes, trunc) = pump(&rx);
        assert!(
            bytes.is_empty() && trunc,
            "拿不到输出要按\"不完整\"报，不许当成功"
        );
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "不许白等 2 秒：实际 {:?}",
            t0.elapsed()
        );
        // 正常回来时原样交出，且不误报截断。
        let (tx2, rx2) = std::sync::mpsc::channel();
        tx2.send((b"hi".to_vec(), false)).unwrap();
        let (bytes, trunc) = pump(&rx2);
        assert_eq!(bytes, b"hi");
        assert!(!trunc);
    }

    /// ★ 码页那条能力**必须由我们自己声明**，不许蹭第三方 crate 的 feature 合并。
    ///
    /// `ansi_to_string` / `GetACP` 用的 `Win32_Globalization` 过去只是恰好被 windui 打开了
    /// 才编得过——那是别人的清单，一次裁剪依赖就会让我们当场编不过（而且报错完全指不到原因）。
    /// 这条盯着 `Cargo.toml`，把"谁提供这只 feature"写回我们自己手里；反向那一格顺手钉住
    /// **已判废的 WinRT OCR 不许被谁再塞回来**（m13 拆掉的那条路）。
    #[test]
    fn codepage_feature_is_pinned_and_winrt_ocr_stays_out() {
        let toml =
            std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
                .expect("读 Cargo.toml");
        assert!(
            toml.contains("Win32_Globalization"),
            "Cargo.toml 必须显式列 Win32_Globalization：`ansi_to_string` / `GetACP` 靠它编译"
        );
        for gone in ["Media_Ocr", "Storage_Streams", "windows-future"] {
            assert!(
                !toml.contains(gone),
                "{gone} 属于已判废的系统 OCR（m13），不该再出现在依赖清单里"
            );
        }
    }

    /// ★ 分层纪律：**只有本模块**允许 `std::process::Command`。
    ///
    /// 现存两处历史调用点在白名单里，各自都有理由；新增调用点必须来说服这条测试
    /// （也就是必须回答"为什么不能走 `platform::process::run`"）。
    #[test]
    fn only_process_module_may_spawn() {
        const ALLOWED: &[(&str, &str)] = &[
            // 打开资源管理器看产物：不是"执行任务"，且只要一个动词 + 一个路径。
            ("src/runtime/overlay.rs", "explorer 打开目录"),
            // 更新器：`.bat` 不是可执行映像，必须经 `cmd /C`（见该文件注释）。
            ("src/update/installer.rs", "cmd /C 跑 .bat"),
        ];
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits: Vec<String> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(code) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if !code.contains("std::process::Command")
                    && !code.contains("use std::process::Command")
                {
                    continue;
                }
                let rel = p
                    .strip_prefix(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == "src/platform/process.rs" {
                    continue;
                }
                if ALLOWED.iter().any(|(f, _)| *f == rel) {
                    continue;
                }
                hits.push(rel);
            }
        }
        assert!(
            hits.is_empty(),
            "这些文件直接用了 std::process::Command：{hits:?}\n\
             请改走 `platform::process::run`（超时 / kill / 截字节都在那里定死了），\n\
             或确有必要时把它加进本测试的 ALLOWED 并写清理由。"
        );
    }
}
