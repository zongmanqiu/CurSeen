//! # 自升级检查链路真网探针（`update_check_probe`）—— 2026-10-08
//!
//! ## 它回答一个问题
//!
//! 「把本机版本临时降到 0.9.0 后，检查更新能否对着 gitee 真实 Release 列表
//! 选中 1.0.0 并拿到正确的 zip 地址」——UI 按钮背后那次网络决策的离线复现。
//! 下载 / 安装（bat 替换 exe）仍需真机走 UI 流程，本探针不碰。
//!
//! ## 做法
//!
//! 直接调生产代码 `checker::fetch_releases()`（真联网，gitee OpenAPI），
//! 再用 `checker::pick_latest` 分别以 0.9.0 / 2.0.0 两个视角判结论：
//!
//! - 0.9.0 视角 → 必须 `Available`，latest=1.0.0，zip = 真实附件
//!   `.../download/1.0.0/CurSeen_1.0.0.zip`（走 assets 命中，不是 fallback 拼接）；
//! - 2.0.0 视角 → 必须 `UpToDate`（gitee 上最新就是 1.0.0）。
//!
//! probe-group: fast（秒级，仅两次字符串判断 + 一次 GET）。

#![cfg(test)]

use curseen_ui::update::checker::{self, CheckOutcome};
use curseen_ui::update::version::Version;

#[test]
#[ignore]
fn real_gitee_selects_1_0_0_for_0_9_and_uptodate_for_2_0() {
    let releases =
        checker::fetch_releases().expect("gitee Release 列表应可获取（需联网）");
    assert!(!releases.is_empty(), "Release 列表不应为空");

    let cur = Version::parse("0.9.0").expect("可解析");
    let outcome = checker::pick_latest(&releases, cur);
    match &outcome {
        CheckOutcome::Available { latest, url } => {
            assert_eq!(latest, "1.0.0", "最新版应是 1.0.0");
            assert!(
                url.starts_with("https://gitee.com/"),
                "更新地址必须来自 gitee 白名单域: {url}"
            );
            assert!(
                url.ends_with("/CurSeen_1.0.0.zip"),
                "应命中真实附件（而非 fallback 拼接）: {url}"
            );
            println!("[probe] 0.9.0 视角 → Available: {url}");
        }
        other => panic!("0.9.0 视角应判 Available，实际 {other:?}"),
    }

    let newer = Version::parse("2.0.0").expect("可解析");
    let outcome = checker::pick_latest(&releases, newer);
    assert_eq!(
        outcome,
        CheckOutcome::UpToDate {
            latest: "1.0.0".into()
        },
        "2.0.0 视角应判已是最新（gitee 上没有更新的 Release）"
    );
    println!("[probe] 2.0.0 视角 → UpToDate（latest 1.0.0）");
}
