//! 安装更新：生成 `curseen_update.bat` 并启动它，由**进程外**完成替换与重启。
//!
//! 为什么必须借道批处理：Windows 不允许覆盖正在运行的可执行文件（`ERROR_ACCESS_DENIED`）。
//!
//! ## 中文路径（用户需求 `更新.txt`）
//!
//! 更新脚本是整条链路里最容易在中文路径上翻车的一环，因为它把路径当**文本**处理。
//! 这里逐条堵住：
//!
//! 1. **不启用 `enabledelayedexpansion`**。原来靠 `!SRCDIR!` 在一个 `for` 块里赋值再读，
//!    而延迟展开会把路径里的 `!` 当变量定界符吃掉——`D:\素材!\CurSeen` 这种目录会静默
//!    变成 `D:\素材\CurSeen`，复制到别的目录去。现在改用 `call :label "%%d"` 传参，
//!    `%~1` 永远是字面量。
//! 2. **文件路径不拼进 PowerShell 命令行**，改走环境变量（`$env:CURSEEN_ZIP`）。命令行是
//!    一层额外的引号/转义解析，路径里有空格、中文、逗号时随时炸；环境变量是 Unicode 的，
//!    原样传递。
//! 3. `chcp 65001` + UTF-8（无 BOM）写脚本，cmd 按 UTF-8 读后续行。
//! 4. 所有路径引用一律加引号（含 `start` 的 `/d` 工作目录）。
//!
//! ## 安全（不是自己的东西不许覆盖）
//!
//! 解压出来的目录若**找不到 exe**，脚本必须中止且**不执行 robocopy**——否则会把一堆
//! 无效解压结果盖到安装目录上，把用户装好的程序砸烂。同时 `robocopy` 显式排除
//! `config.json` / `presets.json` 与 `log` / `update` 目录：配置与日志是**用户的**，
//! 更新包里的同名文件不该覆盖它们。
//!
//! 本模块只做两件有副作用的事：写 bat、起 bat。替换逻辑全在 bat 里，便于出问题时就地排查
//!（用户目录里留着脚本，日志也记了它的路径）。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::paths;

/// bat 文件名（放在程序目录，替换完自己删掉自己）。
pub const BAT_NAME: &str = "curseen_update.bat";

/// 程序所在目录。
pub fn app_dir() -> Result<PathBuf, String> {
    Ok(paths::app_dir())
}

/// 当前可执行文件名（`curseen.exe`）。
///
/// ★ 不能写死：由 `current_exe` 推导——万一将来再改名，更新脚本仍能杀到自己的
/// 进程、也认得出包裹里的 exe。
pub fn exe_name() -> Result<String, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "无法获取可执行文件名".to_string())
}

/// bat 落地路径：`{程序目录}\curseen_update.bat`。
pub fn bat_path() -> Result<PathBuf, String> {
    Ok(app_dir()?.join(BAT_NAME))
}

/// 升级日志：`{程序目录}\log\curseen_update.log`（bat 自己往里追加）。
pub fn log_path() -> Result<PathBuf, String> {
    Ok(paths::log_dir().join("curseen_update.log"))
}

/// 生成更新脚本（纯函数，便于单测）。
///
/// `app` = 程序目录；`zip` = 下载好的更新包；`exe` = 可执行文件名。
pub fn bat_script(app: &Path, zip: &Path, exe: &str) -> String {
    let app = app.to_string_lossy();
    let zip = zip.to_string_lossy();
    // 日志跟着**被更新的程序目录**走，而不是当前进程目录：脚本本身也可能被拿到别的
    // 目录里手工排查。
    let log = format!("{app}\\log");

    let raw = format!(
        r#"@echo off
chcp 65001 >nul
setlocal

REM ── 0. 升级日志：写到 <程序目录>\log\，写不进去就丢弃输出而不是中断更新 ──
set "APP={app}"
set "EXE={exe}"
set "ZIP={zip}"
set "UPDIR=%APP%\update"
set "EXTDIR=%UPDIR%\.extract"
set "LOG={log}\curseen_update.log"
if not exist "{log}" mkdir "{log}" 2>nul
if not exist "{log}" set "LOG=NUL"
if not exist "%APP%" set "LOG=NUL"

call :main >> "%LOG%" 2>&1
exit /b

:main
echo.
echo [%DATE% %TIME%] =============================================
echo [%DATE% %TIME%]  CurSeen Updater
echo [%DATE% %TIME%]  app=%APP%
echo [%DATE% %TIME%]  zip=%ZIP%
echo [%DATE% %TIME%] =============================================

REM ── 1. 关闭正在运行的 CurSeen（文件被占用就没法替换）──
echo [1/5] Closing %EXE% ...
:wait
tasklist /fi "ImageName eq %EXE%" 2>nul | find "%EXE%" >nul
if errorlevel 1 goto kill_done
taskkill /f /im "%EXE%" >nul 2>nul
call :sleep2
goto wait
:kill_done

REM ── 2. 解压 ZIP 到更新目录下的临时文件夹 ──
echo [2/5] Extracting ...
if exist "%EXTDIR%" rmdir /s /q "%EXTDIR%"
mkdir "%EXTDIR%" 2>nul
if not exist "%EXTDIR%" (
    echo ERROR: 无法创建解压目录 "%EXTDIR%"
    goto fail
)
REM 路径经环境变量传给 PowerShell：不进命令行，省掉一整层引号/转义问题。
set "CURSEEN_ZIP=%ZIP%"
set "CURSEEN_DEST=%EXTDIR%"
REM 重试 3 次：刚下载完的 zip 常被杀软/索引程序短暂占用，一次失败就放弃会让用户
REM 每次更新都靠运气。
set "TRY=0"
:extract
set /a TRY+=1
powershell -NoProfile -NonInteractive -Command "Expand-Archive -LiteralPath $env:CURSEEN_ZIP -DestinationPath $env:CURSEEN_DEST -Force -ErrorAction Stop"
if not errorlevel 1 goto extracted
if %TRY% GEQ 3 (
    echo ERROR: 解压失败（包可能不完整、不是 zip，或文件一直被占用）
    goto fail
)
echo    解压失败（第 %TRY% 次），2 秒后重试 ...
call :sleep2
if exist "%EXTDIR%" rmdir /s /q "%EXTDIR%" 2>nul
mkdir "%EXTDIR%" 2>nul
goto extract
:extracted

REM ── 3. 定位内容目录（包内可能多套一层）──
REM 用 call 传参而不是延迟展开：路径里的 "!" 必须原样保留。
set "SRCDIR="
call :probe "%EXTDIR%"
if not defined SRCDIR for /d %%d in ("%EXTDIR%\*") do call :probe "%%d"
if not defined SRCDIR for /d %%d in ("%EXTDIR%\*") do for /d %%e in ("%%d\*") do call :probe "%%e"

if not defined SRCDIR (
    echo ERROR: 更新包里找不到 %EXE%，已中止（未改动任何现有文件）
    goto fail
)
echo [3/5] Source: %SRCDIR%
echo [3/5] Copying files ...

REM ★ 排除用户数据与日志：更新包里的 config.json/presets.json 不该覆盖用户的配置。
robocopy "%SRCDIR%" "%APP%" /e /njh /njs /ndl /nc /ns /np /is /it /xf config.json presets.json /xd log update >nul 2>nul
echo [3/5] robocopy exit=%ERRORLEVEL%

REM ── 4. 清理解压目录（zip 留着，失败时用户能自己解压重试）──
echo [4/5] Cleaning up ...
rmdir /s /q "%EXTDIR%" 2>nul
del /q "%ZIP%" 2>nul
rmdir /s /q "%UPDIR%" 2>nul

REM ── 5. 重新启动并自删 ──
echo [5/5] Starting %EXE% ...
start "" /d "%APP%" "%APP%\%EXE%"
echo [%DATE% %TIME%] Done.
call :sleep2
del "%~f0" >nul 2>nul
exit /b 0

:fail
echo [%DATE% %TIME%] 更新已中止。
rmdir /s /q "%EXTDIR%" 2>nul
REM 不删 zip：用户还能手工解压。
start "" /d "%APP%" "%APP%\%EXE%"
call :sleep2
del "%~f0" >nul 2>nul
exit /b 1

REM ── 工具 ──
:sleep2
REM `timeout` 在没有控制台输入句柄时会直接报错退出，`ping` 是可靠的回退。
timeout /t 2 /nobreak >nul 2>nul || ping -n 3 127.0.0.1 >nul 2>nul
exit /b 0

:probe
if defined SRCDIR exit /b 0
if exist "%~1\%EXE%" set "SRCDIR=%~1"
exit /b 0
"#,
        app = app,
        zip = zip,
        exe = exe,
        log = log
    );
    // 统一成 CRLF：`cmd.exe` 对 LF-only 的批处理容错很差（`goto` 标签会找错行）。
    raw.replace('\n', "\r\n")
}

/// 写 bat 并启动它。返回 bat 路径。
///
/// **成功返回后调用方应当立刻退出进程**（`runtime` 会置退出标志）：bat 的第一步就是在等
/// 我们消失。若不退出，bat 会在 `taskkill` 里把我们杀掉，效果相同，但用户会看到程序"卡"
/// 两秒。
pub fn install(zip: &Path) -> Result<PathBuf, String> {
    let app = app_dir()?;
    let exe = exe_name()?;
    let bat = app.join(BAT_NAME);

    let script = bat_script(&app, zip, &exe);
    std::fs::write(&bat, script.as_bytes()).map_err(|e| format!("写入更新脚本失败: {e}"))?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不弹黑框。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // 必须经 `cmd /C` 启动：`.bat` 不是可执行映像，直接 CreateProcess 会失败。
        Command::new("cmd")
            .arg("/C")
            .arg(&bat)
            .current_dir(&app)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("启动更新脚本失败: {e}"))?;
    }
    #[cfg(not(windows))]
    return Err("仅支持 Windows".to_string());

    log::info!("更新脚本已启动: {}", bat.display());
    Ok(bat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> String {
        bat_script(
            Path::new(r"C:\App\CurSeen"),
            Path::new(r"C:\App\CurSeen\update\curseen_update.zip"),
            "curseen.exe",
        )
    }

    #[test]
    fn script_starts_with_codepage_and_echo_off() {
        let s = script();
        assert!(s.starts_with("@echo off\r\n"), "首行必须是 @echo off");
        assert!(s.contains("chcp 65001"), "中文路径要靠 UTF-8 代码页");
    }

    #[test]
    fn script_uses_crlf_only() {
        let s = script();
        assert!(s.contains("\r\n"));
        assert_eq!(s.matches('\n').count(), s.matches("\r\n").count());
    }

    /// 中文路径最容易翻车的两处：延迟展开吃 `!`、路径拼进 PowerShell 命令行。
    #[test]
    fn script_avoids_delayed_expansion_and_inline_powershell_paths() {
        let s = script();
        assert!(
            !s.contains("enabledelayedexpansion"),
            "不能开延迟展开：路径里的 ! 会被当变量定界符吃掉"
        );
        assert!(!s.contains("!SRCDIR!"), "不该再用 !var! 读路径");
        assert!(
            s.contains(r"$env:CURSEEN_ZIP") && s.contains(r"$env:CURSEEN_DEST"),
            "PowerShell 必须从环境变量取路径，不能把路径拼进 -Command"
        );
        // 确认路径确实没被塞进 PowerShell 命令行
        assert!(!s.contains(r"Expand-Archive -Path '"));
    }

    #[test]
    fn script_covers_all_five_steps() {
        let s = script();
        for step in ["[1/5]", "[2/5]", "[3/5]", "[4/5]", "[5/5]"] {
            assert!(s.contains(step), "缺少步骤 {step}");
        }
        // 关键动作一个都不能少：杀进程 / 解压 / 复制 / 重启。
        assert!(s.contains("taskkill /f /im \"%EXE%\""));
        assert!(s.contains("Expand-Archive"));
        assert!(s.contains("robocopy"));
        assert!(s.contains(r#"start "" /d "%APP%" "%APP%\%EXE%""#));
    }

    /// 找不到 exe 就不许动现有文件——否则会把无效解压结果盖到安装目录上。
    #[test]
    fn script_aborts_before_copy_when_package_has_no_exe() {
        let s = script();
        let guard = s.find("找不到 %EXE%").expect("必须有找不到 exe 的守卫");
        let copy = s.find("robocopy").expect("应有 robocopy");
        assert!(guard < copy, "守卫必须排在 robocopy 之前");
        assert!(s.contains("goto fail"), "失败要跳转到中止分支");
    }

    /// 用户数据不能被更新包覆盖。
    #[test]
    fn script_excludes_user_data_from_robocopy() {
        let s = script();
        let line = s
            .lines()
            .find(|l| l.contains("robocopy"))
            .expect("应有 robocopy 行");
        assert!(
            line.contains("/xf config.json presets.json"),
            "配置要排除: {line}"
        );
        assert!(
            line.contains("/xd log update"),
            "日志与更新目录要排除: {line}"
        );
    }

    /// 解压要能重试：刚下载完的 zip 常被杀软/索引程序短暂占用。
    #[test]
    fn script_retries_extraction() {
        let s = script();
        assert!(s.contains(":extract"), "要有解压重试标签");
        assert!(s.contains("set /a TRY+=1"), "要计数重试次数");
        assert!(s.contains("if %TRY% GEQ 3"), "要有重试上限");
        assert!(s.contains("goto extract"), "失败要回到解压重试");
        assert!(s.contains(":extracted"), "成功后要继续往下走");
    }

    /// 升级日志（用户需求：和 curseen.exe 同级目录增加 log 文件夹）。
    #[test]
    fn script_writes_upgrade_log_under_app_dir() {
        let s = script();
        assert!(s.contains("set \"LOG="), "要有日志路径");
        assert!(s.contains("curseen_update.log"), "日志文件名");
        assert!(
            s.contains(r"C:\App\CurSeen\log\curseen_update.log"),
            "日志要落在被更新程序目录的 log/ 下"
        );
        assert!(
            s.contains(r#"call :main >> "%LOG%" 2>&1"#),
            "主流程要整体重定向进日志"
        );
        assert!(s.contains("set \"LOG=NUL\""), "日志写不进去时不能中断更新");
    }

    /// exe 名不能写死：脚本按调用方传入的实参生成。
    #[test]
    fn script_uses_given_exe_name() {
        let s = bat_script(
            Path::new(r"D:\软件\CurSeen"),
            Path::new(r"D:\软件\CurSeen\update\curseen_update.zip"),
            "curseen.exe",
        );
        assert!(s.contains(r#"set "EXE=curseen.exe""#));
        assert!(!s.contains("curseen2"), "不该残留改名前的旧 exe 名");
        // 中文路径原样写进脚本
        assert!(s.contains(r"D:\软件\CurSeen"));
    }

    #[test]
    fn script_embeds_given_paths() {
        let s = script();
        assert!(s.contains(r"C:\App\CurSeen"));
        assert!(s.contains(r"C:\App\CurSeen\update\curseen_update.zip"));
        // 自删：脚本跑完不留垃圾。
        assert!(s.contains(r#"del "%~f0""#));
    }

    #[test]
    fn app_dir_and_paths_resolve_next_to_the_exe() {
        let exe = std::env::current_exe().expect("可执行路径");
        assert_eq!(app_dir().expect("程序目录"), exe.parent().unwrap());
        assert_eq!(
            bat_path().expect("bat 路径"),
            app_dir().unwrap().join(BAT_NAME)
        );
        assert_eq!(
            log_path().expect("日志路径"),
            paths::log_dir().join("curseen_update.log")
        );
        assert!(!exe_name().expect("exe 名").is_empty());
    }
}
