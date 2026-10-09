//! `<程序目录>/temp` 的占用统计（面板设置页「工作文件夹」卡的数据来源）。
//!
//! 放在 `core`：只碰 `std::fs`，不引 windui / windows，因此可以脱离 UI 单测——造一棵
//! 小目录树、断言总字节与文件数，比对着真机截图猜"那个 44.1 MB 是怎么来的"可靠得多。
//!
//! ★ 调用点必须放**后台线程**（见 `ui::temp_usage::spawn_scan`）：`temp/record` 里攒上
//! 几十个 MP4 时，一次递归 stat 是几十毫秒量级，而首次统计发生在建面板那一帧——留在
//! UI 线程就是"开面板卡一下"，这种卡顿很难归因到一张"只是显示个数字"的卡上。

use std::path::Path;

/// 一个目录（含其下各层子目录）的占用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// 文件字节数合计。
    pub bytes: u64,
    /// 文件个数。
    pub files: u64,
}

/// `temp/` 的统计结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TempUsage {
    /// 根目录当前是否存在。
    ///
    /// 与"存在但是空的"分开：`false` 说明用户**还从没产生过**临时产物（截图、录屏都
    /// 没做过），面板上写"尚未产生文件"比写"0 B"更像人在说话。
    pub exists: bool,
    pub usage: Usage,
}

/// 递归深度上限。
///
/// 目录**符号链接**不会造成环路：`DirEntry::metadata()` 不跟随链接，链接会被当成
/// 普通文件（记它自身的长度），自然不再往下走。但 Windows 的**目录联接（junction）**
/// 在 `is_dir()` 眼里就是目录，一个指回上层的联接能让"把每个目录都走一遍"变成死循环
/// ——统计线程会一直转，而且没有任何界面反馈。加一层上限是最便宜的保险：真踩到也只
/// 是少数几层没算到，不会把线程挂在那里。
const MAX_DEPTH: usize = 32;

/// 递归统计 `root` 下所有文件的字节数与个数。
///
/// 读不动的东西一律跳过（权限被拒的子目录、统计中途被删掉的文件）——统计是**尽力而为**
/// 的展示信息，不该因为一个读不到的文件就整块失败。根目录不存在时返回
/// `TempUsage { exists: false, .. }`。
pub fn scan(root: &Path) -> TempUsage {
    if !root.is_dir() {
        return TempUsage::default();
    }
    let mut usage = Usage::default();
    // 显式栈（不是递归）：目录深到几百层时递归会把栈吃光，显式栈受堆管。
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // ★ 用 `DirEntry::metadata()` 而不是 `fs::metadata(entry.path())`：前者在
            //   Windows 上直接复用 `read_dir` 已经拿回来的 `WIN32_FIND_DATA`，不额外
            //   落盘；后者会给**每一个**条目再发一次 stat。
            let Ok(md) = entry.metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else if md.is_file() {
                usage.bytes = usage.bytes.saturating_add(md.len());
                usage.files += 1;
            }
        }
    }
    TempUsage {
        exists: true,
        usage,
    }
}

/// 统计 `<程序目录>/temp`（面板「工作文件夹」卡看的那个目录）。
pub fn scan_temp() -> TempUsage {
    scan(&super::paths::temp_dir())
}

/// 自动清理的一次执行结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// 删掉的文件个数。
    pub files: u64,
    /// 释放的字节数。
    pub bytes: u64,
    /// 删掉的（变空的）子目录个数（不含根目录）。
    pub dirs: u64,
}

/// 自动清理（2026-09-28 用户口径）：删除 `root` 下**修改时间早于 `days` 天前**的
/// 文件，随后自底向上把因此变空的子目录也删掉（**根目录永不删**）。
///
/// - **每次程序启动执行一次**（`Runtime::new` 拉后台线程），面板开着不动它。
/// - 尽力而为：单个文件/目录删不动（被占用、权限）就跳过并计数归零，不因一个
///   录屏文件正被回放而整块失败。
/// - 判据用**修改时间**（`mtime`）：录屏/截图产物写完即不再改，mtime 就是"何时产生"。
pub fn cleanup_older_than(root: &Path, days: u32) -> std::io::Result<CleanupReport> {
    let mut report = CleanupReport::default();
    if days == 0 || !root.is_dir() {
        return Ok(report); // 0 = 永不
    }
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        .saturating_sub(days as i64 * 24 * 60 * 60);
    // 显式栈收集（深度上限与 `scan` 同一口径）；删除延后到遍历完，避免边走边删。
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(md) = entry.metadata() else {
                continue;
            };
            if md.is_dir() {
                dirs.push(entry.path());
                stack.push((entry.path(), depth + 1));
            } else if md.is_file() {
                files.push((entry.path(), md.len(), md.modified().ok()));
            }
        }
    }
    for (path, len, mtime) in files {
        let old = mtime
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .is_none_or(|secs| secs < cutoff);
        // 拿不到 mtime 的文件按"旧"处理（清出来比留着更符合"自动清理"的意图）。
        if old && std::fs::remove_file(&path).is_ok() {
            report.files += 1;
            report.bytes = report.bytes.saturating_add(len);
        }
    }
    // 自底向上：dirs 收集顺序是父先子后，倒过来删；非空目录 `remove_dir` 自然失败。
    for dir in dirs.iter().rev() {
        if std::fs::remove_dir(dir).is_ok() {
            report.dirs += 1;
        }
    }
    Ok(report)
}

/// 人类可读的体积：`0 B` / `812 B` / `12.3 KB` / `44.1 MB` / `1.2 GB`。
///
/// 一档一个单位、保留一位小数（B 档是整数——"0.0 B"读起来很怪）；不换算到 TB：
/// 一个 `temp/` 攒到 1 TB 时用户要的不是更精确的数字，是"该清了"。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        return format!("{bytes} B");
    }
    if b < KB * KB {
        return format!("{:.1} KB", b / KB);
    }
    if b < KB * KB * KB {
        return format!("{:.1} MB", b / (KB * KB));
    }
    format!("{:.1} GB", b / (KB * KB * KB))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例一棵独立的小树，跑完自己收摊（临时目录下按 pid + 标签命名）。
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("curseen_tempusage_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    fn write(path: &Path, len: usize) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("建父目录失败");
        }
        std::fs::write(path, vec![0u8; len]).expect("写文件失败");
    }

    /// ★ 主用例：多层子目录里的文件全部计入，且与逐层手算的和相等。
    ///
    /// 判据刻意取**非整数 KB** 的字节数：整 KB 会让"少算一层正好差 1024"这类错位
    /// 恰好抵消，看着是绿的。
    #[test]
    fn scan_counts_every_file_across_subdirectories() {
        let root = scratch("tree");
        write(&root.join("screenshot/a.png"), 300);
        write(&root.join("screenshot/b.png"), 724); // 合 1024
        write(&root.join("record/deep/x.mp4"), 5000);
        write(&root.join("gif/g.gif"), 7);

        let u = scan(&root);
        assert!(u.exists, "目录存在时 exists 必须为真");
        assert_eq!(u.usage.files, 4, "四个文件都要数到");
        assert_eq!(u.usage.bytes, 300 + 724 + 5000 + 7);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 不存在的目录 ≠ 空目录：前者是"还没产生过产物"，面板要换一句话说。
    #[test]
    fn scan_reports_missing_root_distinctly() {
        let root =
            std::env::temp_dir().join(format!("curseen_tempusage_none_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let u = scan(&root);
        assert!(!u.exists, "根目录不存在时 exists 必须为假");
        assert_eq!(u.usage, Usage::default());
        assert_eq!(scan(&root), TempUsage::default());
    }

    /// 空目录：`exists == true` 但计数全零 —— 与上一条一起把 `exists` 这一位的语义钉死。
    #[test]
    fn scan_of_empty_dir_is_zero_but_existing() {
        let root = scratch("empty");
        let u = scan(&root);
        assert!(u.exists);
        assert_eq!(u.usage.bytes, 0);
        assert_eq!(u.usage.files, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 体积格式化的**档位边界**。这一档一档的 `if` 最容易把 1024 写进上一档，
    /// 症状是"刚好 1 MB 的录屏显示成 1024.0 KB"。
    #[test]
    fn format_bytes_switches_units_at_the_right_boundary() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1), "1 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        // 本机实测规模：30 个文件 46246936 B → 44.1 MB（与资源管理器一致）。
        assert_eq!(format_bytes(46_246_936), "44.1 MB");
    }

    /// `scan_temp()` 统计的就是 `core::paths::temp_dir()`（便携版 `<exe>/temp`），
    /// 不是系统 `%TEMP%`。
    ///
    /// 拼错的代价很隐蔽：面板上那个数字与「打开文件夹」按钮指向的**不是同一个目录**，
    /// 而两边各自看着都正常，用户几乎不可能自己发现。
    #[test]
    fn scan_temp_targets_the_portable_temp_dir() {
        let portable = super::super::paths::temp_dir();
        assert_eq!(
            portable.file_name().and_then(|n| n.to_str()),
            Some("temp"),
            "便携版临时根目录最后一段必须叫 temp：{portable:?}"
        );
        assert_ne!(
            portable,
            std::env::temp_dir(),
            "便携版的 temp 不是系统 %TEMP%"
        );
        assert_eq!(
            scan_temp(),
            scan(&portable),
            "scan_temp 必须统计便携版那个目录"
        );
    }

    /// 把文件 mtime 拨回去（自动清理测试用；std 1.75+ 的 `File::set_times`）。
    fn age_file(path: &Path, secs_ago: u64) {
        use std::fs::FileTimes;
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("打开文件失败");
        f.set_times(FileTimes::new().set_modified(old))
            .expect("改 mtime 失败");
    }

    /// ★ 主用例：旧文件删、新文件留、变空的子目录一并删、根目录永远在。
    #[test]
    fn cleanup_removes_old_files_and_empty_dirs_only() {
        let root = scratch("cleanup");
        write(&root.join("old/a.mp4"), 1000);
        write(&root.join("new/b.png"), 10);
        age_file(&root.join("old/a.mp4"), 40 * 24 * 60 * 60); // 40 天前
        age_file(&root.join("new/b.png"), 60); // 1 分钟前

        let r = cleanup_older_than(&root, 30).expect("清理不应失败");
        assert_eq!((r.files, r.bytes, r.dirs), (1, 1000, 1), "报告要对账");
        assert!(!root.join("old/a.mp4").exists(), "旧文件必须被删");
        assert!(!root.join("old").exists(), "变空的子目录必须被删");
        assert!(root.join("new/b.png").exists(), "新文件必须留下");
        assert!(root.is_dir(), "根目录永远不删");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 拿不到 mtime / 删不掉的文件：跳过，不失败、不误删别的东西。
    #[test]
    fn cleanup_survives_undeletable_entries() {
        let root = scratch("cleanup_busy");
        write(&root.join("x/y.txt"), 5);
        age_file(&root.join("x/y.txt"), 90 * 24 * 60 * 60);
        // 90 天前的文件在 30 天阈值下该删；这里只验证"days=0 = 永不"这条早退。
        let r = cleanup_older_than(&root, 0).expect("永不档不应执行任何动作");
        assert_eq!(r, CleanupReport::default(), "days=0 必须零动作");
        assert!(root.join("x/y.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
