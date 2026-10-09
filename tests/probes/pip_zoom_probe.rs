//! probe-group: fast
//! 画中画放大「倍率没反应」的真机探针（`#[ignore]`，要真实桌面会话）。
//!
//! # 它要回答的两个问题
//!
//! 用户报"调放大倍率，面板里的画面没有变大"。几何单测（`pip.rs` 里那几条）已经证明
//! `取样区 = 面板 ÷ 倍率` 这条算式没错，所以"没反应"只可能出在**生产链路**上。这条
//! 探针把链路拆成两级，逐级钉死：
//!
//! 1. **成功性**：连续两次 `draw` 之后 `has_frame()` 必须为真 —— 若抓屏或 GDI 呈现
//!    持续失败，面板会定格在**首帧**，于是"套娃消失了（首帧确实干净）"和"倍率没反应
//!    （之后再没换过帧）"会同时成立。这正是最像当前症状的假说。
//! 2. **驱动性**：倍率改了之后，成功出画所用的取样区尺寸必须**真的变小**。
//!
//! 只测几何的测试答不了第 1 条；只测"窗是否可见"的测试答不了第 2 条。

use curseen_ui::core::config::Config;
use curseen_ui::modules::magnifier::pip::PipMagnifier;

/// 造一份"倍率可逐档切换"的配置：`anchor=1`（左上角贴边）——
/// 刻意**不用**鼠标跟随：跟随模式下取样区与面板同心，探针自己很难分辨"抓的是桌面还是
/// 自己的面板"；贴边把面板挪开，取样区落在纯桌面上，判据干净。
fn cfg_at(zoom: u32) -> Config {
    let mut cfg = Config::default();
    cfg.magnifier_pip.enabled = true;
    cfg.magnifier_pip.anchor = 1;
    cfg.magnifier_pip.panel_width = 0.4;
    cfg.magnifier_pip.panel_height = 0.25;
    cfg.magnifier_pip.zoom = zoom;
    cfg
}

/// ★ 假说 1：抓屏 / 呈现根本就没在持续成功（面板定格在首帧）。
#[test]
#[ignore]
fn every_zoom_actually_produces_a_new_frame() {
    let mut m = PipMagnifier::new();
    let mouse = (960.0, 540.0);
    for zoom in [1u32, 2, 3, 4] {
        let cfg = cfg_at(zoom);
        m.draw(&cfg, mouse);
        assert!(
            m.has_frame(),
            "zoom={zoom} 这一档**没有成功出画** —— 面板正定格在首帧，\
             这正是\"套娃没了但倍率也没反应\"的成因"
        );
        println!("zoom={zoom} → 取样区 {:?}", m.last_sample());
    }
}

/// ★ 假说 2：几何没错，但改倍率没有驱动一次新的采样。
#[test]
#[ignore]
fn zoom_drives_the_sample_size_on_the_real_pipeline() {
    let mut m = PipMagnifier::new();
    let mouse = (960.0, 540.0);
    let mut prev: Option<(i32, i32)> = None;
    for zoom in [1u32, 2, 3, 4] {
        let cfg = cfg_at(zoom);
        m.draw(&cfg, mouse);
        let cur = m
            .last_sample()
            .unwrap_or_else(|| panic!("zoom={zoom} 没出画，后面无从比较（先跑上面的成功性用例）"));
        if let Some(p) = prev {
            assert!(
                cur.0 < p.0 && cur.1 < p.1,
                "倍率 {zoom} 没有把取样区缩小：上一档 {p:?} → 本档 {cur:?}"
            );
        }
        prev = Some(cur);
    }
}

/// 鼠标**完全静止**、只改倍率，也必须换帧 —— 复现用户真实操作：
/// 手在控制面板的滑条上（或刚松手），桌面上的光标没动。
#[test]
#[ignore]
fn changing_zoom_alone_refreshes_without_mouse_movement() {
    let mut m = PipMagnifier::new();
    let mouse = (960.0, 540.0);
    m.draw(&cfg_at(2), mouse);
    let before = m.last_sample();
    // 同一坐标、同一切：只有倍率变了。
    m.draw(&cfg_at(4), mouse);
    let after = m.last_sample();
    assert!(
        before != after && after.is_some(),
        "鼠标静止时改倍率没有换帧：{before:?} → {after:?}"
    );
}
