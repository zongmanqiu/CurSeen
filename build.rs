use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // ── exe 资源：图标 + 版本信息 ──
    // 两者都要 Windows SDK 的 `rc.exe`；机器上没有就跳过 —— 缺图标只是"系统默认图标"、
    // 缺版本信息只是"exe 属性里空白"，都不该把构建卡死（CI / 没装 SDK 的机器）。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Some(rc) = find_rc_exe() {
            // 只给 bin 目标（`[[bin]] curseen`）—— 那些 example / probe 不需要这些资源。
            if let Some(res) = compile_icon_resource(&rc) {
                println!("cargo::rustc-link-arg-bins={}", res.display());
            }
            if let Some(res) = compile_version_resource(&rc) {
                println!("cargo::rustc-link-arg-bins={}", res.display());
            }
        }
    }
    println!("cargo:rerun-if-changed=build/app.rc");
    println!("cargo:rerun-if-changed=assets/icons/app.ico");
    // ★ 版本信息的值是**派生**自 Cargo.toml 的（`CARGO_PKG_*`，见
    //   `compile_version_resource`），所以这里必须盯住这几个环境变量：改了 Cargo.toml 的
    //   版本/作者而不重跑本脚本，exe 属性会停在旧值上。
    for key in ["CARGO_PKG_VERSION", "CARGO_PKG_AUTHORS"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    // 把 git 短 hash 与构建时间注入，供"关于"页与日志使用
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CURSEEN_GIT_HASH={hash}");
    // ★ `rerun-if-changed` 指向**不存在的路径**时，cargo 判定"这个依赖永远脏"→
    //   **每次** cargo 调用都重跑本脚本，连带整个 crate 全量重编。
    //   `main` 是工作副本而不是 git 仓库（`git rev-parse` 报 not a git repository），
    //   `.git/HEAD` 根本不存在 —— 实测代价：**连续两次 `cargo test --release --lib`、
    //   零改动，各花 1m12s**（本机 149 个 .rs）。所以这一行必须按存在性下发。
    if std::path::Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
    // 至少钉住本脚本自己：改了它才重跑，其余情况一律复用缓存。
    println!("cargo:rerun-if-changed=build.rs");
}

/// 调 `rc.exe` 把 `src` 编成 `res`；`res` 真的落盘了才算成功。
///
/// `/nologo` 免掉 banner；`/fo` 指定输出 .res。
fn run_rc(rc: &Path, src: &Path, res: &Path) -> bool {
    let ok = Command::new(rc)
        .args(["/nologo", "/fo"])
        .arg(res)
        .arg(src)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && res.exists()
}

/// 把 `build/app.rc` 里的图标编成 .res，返回其路径；任何一步失败都返回 `None`。
///
/// 只在 Windows + MSVC 目标上试：GNU 链的 `.res` 由 `windres` 产出，格式不通用。
fn compile_icon_resource(rc: &Path) -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    let res = out.join("curseen_icon.res");
    if run_rc(rc, Path::new("build/app.rc"), &res) {
        Some(res)
    } else {
        println!(
            "cargo:warning=rc.exe 编译图标资源失败（{}），exe 用系统默认图标",
            rc.display()
        );
        None
    }
}

/// 生成 exe 的**版本信息**（VERSIONINFO）并编成 .res，返回其路径；失败返回 `None`。
///
/// ★ 值**全部取自 Cargo.toml**（`CARGO_PKG_VERSION` / `_AUTHORS`），不在这份 .rc 里写
///   第二份字面量 —— `Cargo.toml` 始终是版本号的唯一来源，不存在"改了版本忘了改资源"
///   的漂移。产物落在 `OUT_DIR`，不污染源码树。
///
/// ★ `FileDescription` 必须是 **`"CurSeen"` 这个字面量**，不要回引
///   `CARGO_PKG_DESCRIPTION`：那是给**源码包**看的描述（本仓现为广告语口径"CurSeen：
///   键鼠所至 · …"），给用户看的进程名/文件描述用它就错了。
///
/// ★★ 更实际的原因：任务管理器「进程」页的**进程名就取自这一行**。缺了它，那里显示的
///   是文件名 `curseen.exe`（小写文件名，用户实报过的体验）。`OriginalFilename` /
///   `InternalName` 写 `curseen`——它们描述的就是文件本身，与 bin 名一致。
fn compile_version_resource(rc: &Path) -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    let src = out.join("curseen_version.rc");
    let res = out.join("curseen_version.res");
    let text = version_rc_text();
    // ★ UTF-8 BOM：`rc.exe` 靠 BOM 认编码。不带 BOM 时它按当前代码页解读，中文会乱码。
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(text.as_bytes());
    if std::fs::write(&src, bytes).is_err() {
        println!("cargo:warning=写版本信息 .rc 失败，exe 属性里将没有公司/版本");
        return None;
    }
    if run_rc(rc, &src, &res) {
        Some(res)
    } else {
        println!("cargo:warning=rc.exe 编译版本资源失败，exe 属性里将没有公司/版本");
        None
    }
}

/// VERSIONINFO 文本。语言块 `0804` = 简体中文、代码页 `1200` = Unicode（UTF-16LE），
/// 与 BOM 编码的中文字符串配套；`FILEOS 0x40004` = VOS_NT_WINDOWS32、`FILETYPE 0x1`
/// = VFT_APP —— 都是 GUI exe 的标准值。
fn version_rc_text() -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let quad = pe_version_quad(&version);
    let author = author_name();
    format!(
        r#"1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "080404b0"
        BEGIN
            VALUE "CompanyName", "{author}"
            VALUE "FileDescription", "CurSeen"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "curseen"
            VALUE "OriginalFilename", "curseen.exe"
            VALUE "ProductName", "CurSeen"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x804, 1200
    END
END
"#
    )
}

/// `"2.0.0"` → `"2,0,0,0"`：VERSIONINFO 的 `FILEVERSION` 要四个逗号分隔的整数，
/// 位数不足补 0（`unwrap_or(0)` 而非 `unwrap`——解析不了就当 0，不能让构建挂掉）。
fn pe_version_quad(v: &str) -> String {
    let mut q = [0u32; 4];
    for (slot, part) in q.iter_mut().zip(v.split('.').take(4)) {
        *slot = part.trim().parse::<u32>().unwrap_or(0);
    }
    format!("{},{},{},{}", q[0], q[1], q[2], q[3])
}

/// `CARGO_PKG_AUTHORS` 形如 `邱宗满 <qiuzongman@foxmail.com>`（多人时以 `:` 分隔），
/// 取第一个人的名字部分（去掉 `<邮箱>`）；取不到就退回产品名。
fn author_name() -> String {
    let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    let first = authors.split(':').next().unwrap_or("").trim();
    let name = first.split('<').next().unwrap_or("").trim();
    if name.is_empty() {
        "CurSeen".to_string()
    } else {
        name.to_string()
    }
}

/// 按 PATH → Windows SDK（装了多个版本时取**最新**的那个）的顺序找 `rc.exe`。
fn find_rc_exe() -> Option<PathBuf> {
    if let Some(p) = which("rc.exe") {
        return Some(p);
    }
    let kits = std::env::var("ProgramFiles(x86)")
        .map(PathBuf::from)
        .ok()?
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(&kits).ok()?.flatten() {
        let ver = entry.file_name().to_string_lossy().into_owned();
        // 目录名是 SDK 版本号（`10.0.26100.0`），字符串序即版本序（同为 10.0.x 时成立）。
        let cand = entry.path().join("x64").join("rc.exe");
        if !cand.exists() {
            continue;
        }
        if best.as_ref().is_none_or(|(v, _)| ver > *v) {
            best = Some((ver, cand));
        }
    }
    best.map(|(_, p)| p)
}

/// PATH 里找可执行文件（自带的极简版：只需要 `rc.exe` 一个，不值得为它引依赖）。
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var("PATH").ok()?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .find(|p| p.exists())
}
