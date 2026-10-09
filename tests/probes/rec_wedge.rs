//! 楔死注入判据：录制线程被楔住时，软件侧必须**限时拿到如实结论、且自己照常活着**。
//!
//! 跑法：
//!
//! ```text
//! python build/probe.py run rec_wedge
//! REC_WEDGE_AT=finalize python build/probe.py run rec_wedge
//! ```
//!
//! 环境变量：`REC_WEDGE_AT` = `encode`（默认）/ `acquire` / `finalize`；
//! `REC_WEDGE_FINISH_SECS` = 收尾限时（默认 3）。
//!
//! # 为什么要有这条判据
//!
//! 2026-10-06 的 7200s 联调长测暴露了"收尾超时被静默吞掉"这条真 bug：录制线程楔住时
//! 结论里 `error = None`，界面会把一个**没有 moov、打不开**的产物当成"录制完成"。
//! 那次的修法是"超时如实报错"，但"如实报错"本身也需要一条会红的判据来钉住。
//!
//! 三条断言对应两条原则性要求：
//!   1. **软件绝不因录屏卡死** —— `finish_detached` 的回调必须到达，且总耗时受控
//!      （远小于"无限等待"）；同时另起一个"UI 侧"计数线程，验证主流程没被阻塞。
//!   2. **绝不能把失败说成完成** —— 结论必须 `wedged == true`、错误里带阶段名。
//!
//! # ★★ `--test-threads=1`
//! DXGI 桌面复制同一时刻只容一份；而且本用例**故意留下一个永不退出的录制线程**
//! （楔住就是楔住），它会一直占着复制对象直到进程结束 —— 所以它只能单独跑。

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{self, RecOptions};

/// 收尾限时（秒）：故意给得很短，让"楔住"在可控时间内暴露。
fn finish_secs() -> u64 {
    std::env::var("REC_WEDGE_FINISH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

#[test]
#[ignore]
fn wedged_recorder_never_drags_the_ui_down() {
    let _ = env_logger::builder().is_test(false).try_init();

    let stage = std::env::var("REC_WEDGE_AT").unwrap_or_else(|_| "encode".to_string());
    let finish_secs = finish_secs();
    // 录制线程在 `run_mp4` 里读这个变量 —— 必须在 `Recorder::start` **之前**设好。
    std::env::set_var("CURSEEN_REC_WEDGE_AT", &stage);

    let out = std::env::temp_dir().join("curseen_wedge.mp4");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 0,
        hardware: recorder::Hardware::Auto,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");

    // ── "UI 侧"心跳：一个独立计数线程，用来证明主流程没被录制侧阻塞 ──
    let ui_ticks = Arc::new(AtomicU64::new(0));
    let ui_stop = Arc::new(AtomicU64::new(0));
    let t_ticks = ui_ticks.clone();
    let t_stop = ui_stop.clone();
    std::thread::spawn(move || {
        while t_stop.load(Ordering::Relaxed) == 0 {
            t_ticks.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    // ── 等"楔住真的发生" ──
    // ★ 注入点是第 11 帧（`maybe_wedge` 的 `tick >= 10`）：等得太早（只看帧数够不够）
    //   就会在注入生效**之前**请求停止，线程正常退出，判据变成"测了个寂寞"
    //   （2026-10-06 第一次跑就是这样红的：收尾 54ms、wedged=false）。
    //   所以非 finalize 阶段要等**帧数冻住**——那才是楔住的证据。
    let is_finalize = stage.eq_ignore_ascii_case("finalize");
    // ★★ 冻结判据看**已写入帧数**（`written()`），不是采集帧数（`frames()`）。
    //   采集/编码解耦（2026-10-06）之后，`encode` 楔住时**采集侧照常出帧** —— 那正是
    //   解耦要达到的效果（软件不被编码器拖住），但也意味着用 `frames()` 永远等不到
    //   冻结：判据会变成"注入没生效"的假红。真正冻住的是"写进文件的帧数"。
    let t0 = Instant::now();
    let mut last = rec.written();
    let mut stable_since = Instant::now();
    let mut frozen = false;
    while t0.elapsed() < Duration::from_secs(20) {
        let f = rec.written();
        if f != last {
            last = f;
            stable_since = Instant::now();
        } else if f >= 8 && stable_since.elapsed() > Duration::from_millis(1500) {
            frozen = true;
            break;
        }
        // `finalize` 阶段是在收尾时才楔，帧数会一直涨到停止位，所以只等够帧数。
        if is_finalize && f >= 20 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let frames_before = rec.frames();
    let written_before = rec.written();
    println!(
        "楔住前：采集 {frames_before} 帧 / 已写入 {written_before} 帧 / 丢弃 {} 帧（注入点 = {stage}，写入冻结 = {frozen}）",
        rec.dropped()
    );
    assert!(
        written_before >= 8,
        "注入前应能正常写入 8 帧以上，实际 {written_before}（录制起不来？）"
    );
    if !is_finalize {
        assert!(
            frozen,
            "写入帧数仍在推进（{written_before}）——楔死注入没生效，这条判据会变成假绿"
        );
        // ★ 解耦的正面证据：`encode` 楔住时采集侧**必须**还在出帧（否则说明
        //   "采集被编码器拖住"这条老毛病又回来了）。
        if stage.eq_ignore_ascii_case("encode") {
            assert!(
                frames_before > written_before,
                "encode 楔住时采集侧应继续出帧（采集 {frames_before} / 写入 {written_before}）——\
                 采集被编码拖住了，解耦没生效"
            );
        }
    }

    // ── 判据 1：收尾限时返回，UI 侧照常推进 ──
    let (tx, rx) = mpsc::channel();
    let t1 = Instant::now();
    rec.finish_detached(Duration::from_secs(finish_secs), move |o| {
        let _ = tx.send(o);
    });
    // 回调"一定"会到（含超时弃子那条路）；给限时 + 10s 的宽限，超过就是真拖死了。
    let budget = Duration::from_secs(finish_secs + 10);
    let done = rx
        .recv_timeout(budget)
        .unwrap_or_else(|_| panic!("收尾回调在 {budget:?} 内没到 —— UI 被录制侧拖死了"));
    let waited = t1.elapsed();
    ui_stop.store(1, Ordering::Relaxed);
    let ticks = ui_ticks.load(Ordering::Relaxed);

    println!(
        "收尾回调耗时 {waited:?}（限 {finish_secs}s）；期间 UI 侧心跳 {ticks} 次"
    );
    assert!(
        waited < budget,
        "收尾耗时 {waited:?} 超过预算 {budget:?}：软件被录制收尾拖住了"
    );
    // 20ms 一次 → 至少该有"等待秒数 × 20"次的一半（留一半给调度抖动）。
    let expected = (waited.as_millis() / 20) as u64 / 2;
    assert!(
        ticks >= expected.max(5),
        "UI 侧心跳只有 {ticks} 次（期望 ≥ {expected}）：主线程被阻塞了"
    );

    // ── 判据 2：结论如实（楔住 ≠ 完成）──
    println!(
        "结论：bytes={:?} wedged={} error={:?}",
        done.bytes, done.wedged, done.error
    );
    assert!(
        done.wedged,
        "录制线程被楔住，结论必须是 wedged=true（否则界面又会把打不开的产物说成完成）"
    );
    assert!(
        !done.is_clean(),
        "楔住的录制绝不能 is_clean()（这是 2026-09-19 同款事故的判断点）"
    );
    let err = done.error.as_deref().unwrap_or_default();
    // ★ 文案按载体分两种：进程内线程说"收尾超时"，子进程说"未退出（已强杀）"。
    //   判据认**语义**（收尾到点没回来 + 说了原因），不认具体措辞 —— 否则换一次实现
    //   就要改一次判据，而真正要保证的是"绝不当成完成上报"。
    assert!(
        err.contains("收尾超时") || err.contains("未退出"),
        "错误信息应说明是收尾超时/子进程未退出，实际：{err}"
    );
    println!("OK：楔住被如实判为中断，软件侧未受拖累（{err}）");
    // ★ 2026-10-07 起录制收回进程内线程（不再出进程）：楔住的线程杀不掉，桌面复制
    //   单例被占 ⇒ 只能重启软件（`runtime::record` 如实上报）。"强杀后再录"判据随之
    //   作废删除 —— 它钉的是出进程隔离独有的能力。
    std::env::remove_var("CURSEEN_REC_WEDGE_AT");
