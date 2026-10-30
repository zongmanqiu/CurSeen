//! 监听整个 `assets/` 的目录变化（m35 起 `assets/lua`，2026-10-29 扩到 `assets/`）：
//! 脚本、虚拟键盘方案、声音文件放进/删除/解压出来，自动触发一次重扫。
//!
//! # 为什么是"监听 + 脏标记"，不是"定时扫盘"
//!
//! 扫一遍 `assets/lua`（163 个脚本）实测约 55 ms；按 250 ms 周期轮询就是每秒几百次
//! `stat` + 四次全扫，空闲 CPU 与磁盘都被白白唤醒——反着"低占用"那把尺子。
//! `ReadDirectoryChangesW` 是**阻塞**的：没有变化时线程睡在内核里，零 CPU。
//!
//! 监听线程只做一件事：`mark_dirty()`（一个原子写）。真正的重扫发生在 UI 线程的
//! 250 ms 对账里（[`crate::runtime::Runtime::refresh_assets_if_changed`]）——
//! 那里天然把一阵突发合流成一次，也不在监听线程上碰 `PanelModel`（它不是 `Send`）。
//!
//! 失败（目录不存在、权限不足）只记一行日志：脚本页仍可重开面板刷新，不该因为
//! 监听起不来就挡住启动。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// "脚本目录变过了"的进程级标记。**只放这里**：监听线程写、UI 对账读，其它地方不碰。
static DIRTY: AtomicBool = AtomicBool::new(false);

/// 标记"目录变了"（监听线程用；测试也用它模拟一次变化）。
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Release);
}

/// 取走标记（取过即清）。返回 `true` = 自上次消费以来变过。
pub fn take_dirty() -> bool {
    DIRTY.swap(false, Ordering::AcqRel)
}

/// 看一眼标记但**不清**。给"现在不宜扫盘"的场合用（例如正在录屏）：标记留着，
/// 等条件允许时再由 [`take_dirty`] 消费——迟到的刷新总比抖一下录制好。
pub fn dirty() -> bool {
    DIRTY.load(Ordering::Acquire)
}

/// 在**后台线程**上监听 `dir`（递归）。返回是否成功起了监听。
///
/// `label` 只用于日志（"assets/lua 目录监听结束：…"）。线程随进程退出，不需要停。
pub fn watch_dir(dir: &Path, label: &'static str) -> bool {
    let dir = dir.to_path_buf();
    let (opened_tx, opened_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("fx-assets-watch".into())
        .spawn(move || watch_loop(&dir, label, Some(opened_tx)));
    match spawned {
        Ok(_) => {
            // ★ 等线程真的拿到目录句柄再返回：只 spawn 就返回的话，调用方紧接着写文件
            //   的那条事件可能落在句柄建好之前 ⇒ 漏报。整门禁高负载时这条竞态偶发
            //   （m38 复现），启动那一拍也是同一件事。目录不存在 ⇒ 线程退出、通道断开
            //   ⇒ 立刻返回 false（不白等 2 s）。
            opened_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok()
        }
        Err(e) => {
            log::warn!("起 {label} 目录监听线程失败：{e}（脚本页仍可重开面板刷新）");
            false
        }
    }
}

fn watch_loop(dir: &Path, label: &'static str, opened: Option<std::sync::mpsc::Sender<()>>) {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` 是栈上以 NUL 结尾的缓冲，只在建句柄期间被系统读。
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            log::warn!(
                "打开 {label} 目录失败（{}）：{e}——自动刷新不可用",
                dir.display()
            );
            return;
        }
    };
    // 句柄已建好：叫醒 `watch_dir` 的调用方（之后的事件一条都不会漏）。
    if let Some(tx) = opened {
        let _ = tx.send(());
    }
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let mut returned = 0u32;
        // 阻塞调用：没有变化就一直睡在内核里（零 CPU）。递归监听子目录——
        // 用户往 `scripts/<包>/` 里丢文件也要算。
        let r = unsafe {
            windows::Win32::Storage::FileSystem::ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE,
                Some(&mut returned),
                None,
                None,
            )
        };
        match r {
            Ok(()) => {
                if returned > 0 {
                    mark_dirty();
                }
            }
            Err(e) => {
                log::warn!("{label} 目录监听结束：{e}——自动刷新不可用（面板可重开）");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("curseen_watch_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wait_dirty(timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if take_dirty() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// ★ 新建要能报、删除也要能报（用户"删掉一个脚本"与"放进一个脚本"是同一件事）。
    /// 这格走真 `ReadDirectoryChangesW`（不是模拟），是本模块唯一的机器读数。
    #[test]
    fn new_and_deleted_files_both_mark_dirty() {
        let d = tmpdir("both");
        assert!(watch_dir(&d, "测试目录"), "监听线程该起得来");
        let _ = take_dirty(); // 清掉别的测试可能留下的
        std::fs::write(d.join("a.lua"), b"-- x").unwrap();
        assert!(
            wait_dirty(std::time::Duration::from_secs(5)),
            "新建文件没有报上来"
        );
        std::fs::remove_file(d.join("a.lua")).unwrap();
        assert!(
            wait_dirty(std::time::Duration::from_secs(5)),
            "删除文件没有报上来"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `dirty()` 是"看一眼不清"：延后刷新的场景要反复看，不能看一次就把标记吃掉。
    #[test]
    fn peek_dirty_does_not_consume_the_flag() {
        let _ = take_dirty();
        mark_dirty();
        assert!(dirty(), "标记该看得见");
        assert!(dirty(), "看一眼不许把它清掉");
        assert!(take_dirty(), "真正消费时才拿走");
        assert!(!dirty(), "消费之后就该是干净的");
    }

    /// 监听一个**不存在**的目录：不许 panic、不许卡住，返回 false。
    #[test]
    fn watching_a_missing_dir_fails_softly() {
        let missing = std::env::temp_dir().join("curseen_watch_missing_dir_xyz");
        let _ = std::fs::remove_dir_all(&missing);
        // 线程会起来、建句柄失败后立刻退出；`watch_dir` 靠通道断开**立刻**返回 false，
        // 不会白等 2 s（这条同时钉住"返回值 = 真的监听着"）。
        assert!(!watch_dir(&missing, "不存在的目录"));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = take_dirty();
    }
}
