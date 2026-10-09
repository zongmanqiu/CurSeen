//! 截图 / 录屏的冒烟测试。
//!
//! 都是 `#[ignore]`：它们会真的抓屏、真的写文件，不适合进 `cargo test` 的默认集合
//! （CI 上多半没有可用的桌面会话）。本地验证时手工跑：
//!
//! ```text
//! cargo test --release --test capture_smoke -- --ignored --nocapture --test-threads=1
//! ```
//!
//! # ★★ `--test-threads=1` 不是可选项
//!
//! DXGI Desktop Duplication **同一时刻只容得下一份**（实测见
//! `platform::recorder::DuplHolder` 的文档注释）：手上有旧的再建新的必然得
//! `0x80070057`，而且它**不会把先前的挤掉**。cargo 默认并行跑测试，于是三个录制用例
//! 互相抢同一份复制对象：实测 GIF 那个赢了、两个 MP4 用例双双挂在
//! `建立桌面复制被拒：同一时刻只容一份桌面复制` —— 看着像"录屏坏了"，
//! 其实只是并发。串行跑就全过。

#![cfg(windows)]

use std::sync::Once;
static INIT: Once = Once::new();
fn init() {
    INIT.call_once(|| {
        let _ = env_logger::builder().is_test(false).try_init();
    });
}

/// 清掉可能残留的故障注入开关。
///
/// 环境变量是**进程级**的，而 cargo 在同一个进程里跑本文件全部用例：`aborted_*`
/// 那个用例设过 `CURSEEN_REC_FAULT_AFTER`，一旦它提前 panic（没走到清理那行），
/// 后面的"完整录制"用例就会莫名报出注入的中断。所以正常的录制用例开跑前先扫一遍。
fn clear_fault_injection() {
    std::env::remove_var("CURSEEN_REC_FAULT_AFTER");
}

use std::time::Duration;

use curseen_ui::platform::recorder::{self, RecOptions};
use curseen_ui::platform::screenshot;

// 失效重建那条真机探针要直接握 DXGI 接口（`recorder` 没把 `DuplHolder` 的内部暴露出来，
// 而这里要验的正是"驱动认不认这个顺序"，不是 Rust 侧的记账）。
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

#[test]
#[ignore]
fn screenshot_writes_a_real_png() {
    let rect = curseen_ui::platform::display::virtual_screen();
    let shot = screenshot::grab(rect).expect("抓屏应成功");
    let out = std::env::temp_dir().join("curseen_smoke.png");
    screenshot::save_png(&shot, &out).expect("存 PNG 应成功");
    let meta = std::fs::metadata(&out).expect("文件应在");
    println!(
        "截图 {}x{} -> {} ({} 字节)",
        shot.w,
        shot.h,
        out.display(),
        meta.len()
    );
    assert!(meta.len() > 1024, "PNG 不可能只有 {meta:?} 字节");
}

#[test]
#[ignore]
fn record_writes_a_playable_mp4() {
    init();
    clear_fault_injection();
    let out = std::env::temp_dir().join("curseen_smoke.mp4");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        // 原始分辨率 + 自动硬件加速：这两个是"和以前一样"的基线，改动它们会改变
        // 这条烟测的实际负载（缩放在 GPU 上做，影响帧率）。
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
    // ★ 计时从**第一帧**起算，不是从 `start()` 返回起算：后台线程起来后还要建 D3D 设备
    //   / 桌面复制 / 编码器，实测约 0.5 秒。这段是启动开销，不该算进"录了多久"。
    let t0 = wait_first_frame(&rec);
    std::thread::sleep(Duration::from_secs(12));
    let real = t0.elapsed().as_secs_f64();
    let frames = rec.frames();
    let done = rec.finish();
    assert!(done.is_clean(), "完整录制不该带错误: {:?}", done.error);
    let size = done.bytes.unwrap_or(0);
    println!(
        "录屏 {frames} 帧 / 真实 {real:.2}s -> {} ({size} 字节)",
        out.display()
    );

    // 有帧 + 有 'ftyp' box（MP4 文件头）才算真的写出来了。
    let head = std::fs::read(&out).expect("应能读回");
    assert!(frames > 10, "3 秒 30fps 至少该有十几帧，实际 {frames}");
    assert!(
        head.windows(4).any(|w| w == b"ftyp"),
        "缺少 MP4 文件头 ftyp"
    );
    // ★ 必须真的收尾过：`moov` 是 `Finalize` 才写的索引盒子。缺了它就是"有几 MB、
    //   放不了"的幌子（见 `Recorder` 的 `FinishOutcome` 与 `SinkFinisher`）。
    assert!(
        head.windows(4).any(|w| w == b"moov"),
        "缺少 moov —— 说明没走到 Finalize，文件不可播放"
    );
    assert!(
        head.windows(4).any(|w| w == b"avc1"),
        "缺少 H264 (avc1) 编码轨"
    );

    // 时长必须贴合真实经过的时间：少录几帧没关系，但"3 秒录出 2.4 秒的视频"
    // 是肉眼可见的缺陷（画面整体变快 20%）。
    let secs = mp4_seconds(&head).expect("应能读出 mvhd");
    println!(
        "MP4 时长 {secs:.2}s（真实 {real:.2}s，{frames} 帧 → {:.1}fps）",
        frames as f64 / real
    );
    assert!(
        (secs - real).abs() < 0.25,
        "时长应约等于真实 {real:.2}s，实际 {secs:.2}s"
    );
    assert!(size > 4096, "视频不可能只有 {size} 字节");
}

/// GIF 通路烟测：`RecKind::Gif` 走的是与 MP4 **同一条采集会话**，只换封装器。
///
/// 钉三件事：
/// 1. 文件真的是 GIF（`GIF89a` 魔数 + 结尾 `0x3B` 的 trailer）；
/// 2. 帧数是真的在涨（采集循环没在 GIF 分支里退化成"一帧就停"）；
/// 3. 逻辑屏幕尺寸 = 档位换算出来的偶数尺寸（宽度/高度从逻辑屏幕描述符里读）。
///
/// 为什么这条不能塞进 `cargo test` 默认跑：它要真的开一个 DXGI 桌面复制会话，
/// 无头/远程桌面下会失败 —— 与同文件的其它烟测一样标 `#[ignore]`。
#[test]
#[ignore]
fn record_writes_a_real_gif() {
    init();
    clear_fault_injection();
    let out = std::env::temp_dir().join("curseen_smoke.gif");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Gif,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 15,
        bitrate_kbps: None, // 自动
        // 1280 档（长边）：让缩放通路也被走到。
        max_long_edge: 1280,
        hardware: recorder::Hardware::Off,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起 GIF 录制");
    let t0 = wait_first_frame(&rec);
    std::thread::sleep(Duration::from_secs(2));
    let real = t0.elapsed().as_secs_f64();
    let frames = rec.frames();
    let done = rec.finish();
    assert!(done.is_clean(), "完整 GIF 录制不该带错误: {:?}", done.error);
    let size = done.bytes.unwrap_or(0);
    println!(
        "GIF {frames} 帧 / 真实 {real:.2}s -> {} ({size} 字节)",
        out.display()
    );

    let bytes = std::fs::read(&out).expect("应能读回 GIF");
    assert!(
        bytes.starts_with(b"GIF89a"),
        "缺少 GIF89a 文件头：{:?}",
        &bytes[..6.min(bytes.len())]
    );
    assert_eq!(
        *bytes.last().expect("非空"),
        0x3B,
        "GIF 必须以 trailer 0x3B 收尾（缺了说明没走 finish）"
    );
    assert!(frames > 15, "2 秒 15fps 至少该有二十几帧，实际 {frames}");
    assert!(size > 1024, "GIF 不可能只有 {size} 字节");
    // 逻辑屏幕描述符在 'GIF89a'(6) 之后：宽 2 字节 + 高 2 字节（小端）。
    let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    println!("GIF 尺寸 {w}×{h}");
    assert_eq!(w % 2, 0, "宽必须是偶数（H264/GIF 共用取偶规则）");
    assert_eq!(h % 2, 0, "高必须是偶数");
    // 只断言"不超过档位上限"：屏幕本身比 1280 小的时候是**不上采样**的（与录屏同规则），
    // 此时长边就是屏宽，写成等号会在小屏机器上误报。
    assert!(w.max(h) <= 1280, "长边不该超过 1280 档的上限，实际 {w}×{h}");
}

/// ★★★ 真机验证「失效重建」这条路的顺序约束。
///
/// `DuplHolder::replace` 的门禁是纯逻辑（假对象记账），它钉得住"先放后建"，但证明不了
/// **真驱动也这么判**。这条用例直接对 DXGI 走一遍失效重建会走的那个序列：
///
/// ```text
/// 1. 建第一份 duplication                       → 必须成功
/// 2. 手上有第一份时再建第二份（模拟"先建后丢"） → 必须失败，且失败码是那两个"不容并存"
/// 3. 放掉第一份后再建（模拟 `replace` 的顺序）   → 必须成功
/// ```
///
/// 第 2 步失败、第 3 步成功，就是驱动对"先丢后建"这条路的背书；反过来若第 3 步也失败，
/// 那说明真机上还存在别的占用者（别的进程 / 另一个 `Session`），重建策略得另想办法。
///
/// 必须串行跑（见文件头的说明），且要真实桌面会话，所以标 `#[ignore]`。
#[test]
#[ignore]
fn rebuilding_a_duplication_needs_the_old_one_gone_first() {
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };

    /// "同一时刻只容一份"的两种实际返回值（与 `recorder.rs` 里的判据同源）。
    const FATAL: [windows::core::HRESULT; 2] = [
        windows::core::HRESULT(0x887A_0007u32 as i32),
        windows::core::HRESULT(0x8007_0057u32 as i32),
    ];

    unsafe {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .expect("应能建 D3D 设备");
        let device: ID3D11Device = device.expect("建设备不应返回空句柄");
        let dxgi_device: IDXGIDevice = device.cast().expect("应能取 IDXGIDevice");
        let adapter: IDXGIAdapter = dxgi_device.GetAdapter().expect("应能取适配器");
        let output: IDXGIOutput1 = pick_first_output(&adapter).expect("应能找到显示器输出");

        // ── 1. 第一份必须成功 ──
        let first = output
            .DuplicateOutput(&device)
            .expect("第一份桌面复制应当建得起来");
        println!("第一份 duplication 建好");

        // ── 2. 手上还拿着它时再建一份：必须被拒 ──
        let second = output.DuplicateOutput(&device);
        match &second {
            Ok(_) => panic!(
                "手上还活着第一份时竟然建出了第二份 —— 那 '先建后丢' 是可行的，\
                 recorder 里的顺序约束和门禁都该重新审"
            ),
            Err(e) => {
                println!("第二份如期被拒: {e:?} (0x{:08X})", e.code().0);
                assert!(
                    FATAL.contains(&e.code()),
                    "拒绝的理由应当是'不容并存'，实际 0x{:08X} —— \
                     若换了别的码，说明约束的成因不是'两份并存'",
                    e.code().0
                );
            }
        }

        // ── 3. 放掉第一份后再建：必须成功（这就是 `replace` 走的顺序）──
        // 持有一帧不放再丢弃是驱动记账的一部分，所以连 `ReleaseFrame` 一起走掉 ——
        // 与 `DuplHolder::replace` 里做的事逐字对应。
        let _ = first.ReleaseFrame();
        drop(first);
        let third = output
            .DuplicateOutput(&device)
            .expect("先释放旧的之后必须能重建 —— 这正是 DuplHolder::replace 依赖的前提");
        let desc = third.GetDesc();
        println!("重建成功: {}x{}", desc.ModeDesc.Width, desc.ModeDesc.Height);
        assert!(
            desc.ModeDesc.Width > 0,
            "重建出来的复制对象必须报得出有效尺寸"
        );
        let _ = third.ReleaseFrame();
    }
}

/// 取第一个可用的输出（跳过 `EnumOutputs` 的第一个失败项即可，本机单屏场景足够）。
unsafe fn pick_first_output(adapter: &IDXGIAdapter) -> Option<IDXGIOutput1> {
    for i in 0..4 {
        if let Ok(o) = adapter.EnumOutputs(i) {
            if let Ok(o1) = o.cast::<IDXGIOutput1>() {
                return Some(o1);
            }
        }
    }
    None
}

/// ★★ 中断的录制也必须留下**可播放**的产物。
///
/// 这是 2026-09-19 的真实现场：录到第 8 秒 `ProcessOutput` 报
/// `GPU 设备实例已经暂停 (0x887A0005)`，线程带着 `Err` 返回，于是
/// `IMFSinkWriter::Finalize` **从没被调用** —— 产物 3,518,527 字节，`ftyp` 之后直接是
/// `mdat`、**没有 `moov`**，任何播放器都打不开；而界面照样弹"录制完成"。
///
/// 真现场靠硬件抽风，没法按需复现，所以用 `CURSEEN_REC_FAULT_AFTER=N` 在第 N 帧后
/// 注入一次错误（见 `recorder.rs` 里那段注释）。这条测试同时钉两件事：
///   1. 产物仍有 `moov` + H264 轨（= `SinkFinisher` 生效，短片能播）；
///   2. 结论**不能**是 `is_clean()` —— 否则界面又把"中断"说成"完成"。
#[test]
#[ignore]
fn aborted_record_leaves_a_playable_mp4() {
    init();
    let out = std::env::temp_dir().join("curseen_abort_smoke.mp4");
    let _ = std::fs::remove_file(&out);

    std::env::set_var("CURSEEN_REC_FAULT_AFTER", "20");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 0,
        hardware: recorder::Hardware::Auto,
        show_cursor: false,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");
    let _ = wait_first_frame(&rec);
    // 20 帧 @30fps ≈ 0.7s；给足余量让线程撞上注入点并退出。
    std::thread::sleep(Duration::from_secs(3));
    // ⚠ 这里**不能**先 `take_error()` 探活：那个方法"取走即无"，会把错误串吃掉，
    //   随后 `finish()` 就看不到错误、误判成"完整录完"。
    let done = rec.finish();
    // ★★ 立刻清掉注入开关：环境变量是**进程级**的，而 cargo 在同一个进程里跑本文件的
    //    所有用例 —— 漏了这一步，后面 `record_writes_a_playable_mp4` 会莫名其妙地
    //    在"完整录制"里报出注入的中断（实测就是这么红的）。放在断言之前，panic 也不漏。
    std::env::remove_var("CURSEEN_REC_FAULT_AFTER");
    println!(
        "中断产物 {} 字节（注入点=第 20 帧）：{:?}",
        done.bytes.unwrap_or(0),
        done.error
    );

    assert!(
        !done.is_clean(),
        "注入了错误却被判成『完整录完』——界面就又会骗用户"
    );
    assert!(
        done.error.is_some(),
        "错误串必须交出来，否则界面没法说清『这段是中断的』"
    );
    let bytes = std::fs::read(&out).expect("产物文件应存在");
    assert!(
        bytes.windows(4).any(|w| w == b"moov"),
        "缺少 moov：中断后没有收尾，文件不可播放（{} 字节）",
        bytes.len()
    );
    assert!(
        bytes.windows(4).any(|w| w == b"avc1"),
        "缺少 H264 (avc1) 轨"
    );
    assert!(
        done.bytes.unwrap_or(0) > 4096,
        "20 帧 1080p 不该只有 {} 字节",
        done.bytes.unwrap_or(0)
    );
}

/// ★★ 后台收尾（`finish_detached`）与同步 `finish` 产出**同等可用**的产物。
///
/// 2026-10-02 起停止录制不在 UI 线程收尾（UI 只发停止位，收尾在后台线程限时 join，
/// 防止编码器卡住把整个程序冻结）。这条烟测钉住改造后的契约：
///   1. `on_done` 恰好被调用一次，结论是干净的（`is_clean`）；
///   2. 产物同样有 `moov`（= 后台路径也走到了 `Finalize`）。
#[test]
#[ignore]
fn finish_detached_finalizes_like_sync_finish() {
    init();
    clear_fault_injection();
    let out = std::env::temp_dir().join("curseen_smoke_detached.mp4");
    let _ = std::fs::remove_file(&out);
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 0,
        hardware: recorder::Hardware::Auto,
        show_cursor: false,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");
    let _ = wait_first_frame(&rec);
    std::thread::sleep(Duration::from_secs(2));

    let (tx, rx) = std::sync::mpsc::channel();
    rec.finish_detached(Duration::from_secs(30), move |o| {
        let _ = tx.send(o);
    });
    // 调用线程立刻继续（这正是改造点：不阻塞"UI 线程"），只在测试里等结论。
    let done = rx
        .recv_timeout(Duration::from_secs(35))
        .expect("收尾线程应交回结论");
    println!(
        "后台收尾产物 {} 字节：{:?}",
        done.bytes.unwrap_or(0),
        done.error
    );
    assert!(
        done.is_clean(),
        "完整录制经后台收尾不该带错误: {:?}",
        done.error
    );
    let bytes = std::fs::read(&out).expect("产物文件应存在");
    assert!(
        bytes.windows(4).any(|w| w == b"moov"),
        "后台收尾的产物缺 moov —— 说明 Finalize 没跑到"
    );
}

/// ★★★ 录制线程**卡死在采集步**时，产物也必须能播（2026-10-02 现场回归锁）。
///
/// 现场：线程楔在 `AcquireNextFrame` 里不返回 —— 不报错、不写帧、发停止位也唤不醒。
/// 此时 `SinkFinisher` 守卫还压在卡死线程自己的栈上，栈永不展开、`Finalize` 永不执行
/// ⇒ 产物必然是"几百 MB、`mdat` 之后什么都没有"的废文件（当天 177MB / 8.8MB 两次皆然）。
///
/// 修复：`IMFSinkWriter` 另外登记到 `Recorder` 的 `SinkSlot` 上；收尾线程确认"卡在采集
/// 侧"（此刻线程不可能在 `WriteSample` 里）就自己接管 `Finalize`，把卡住前的部分补上索引。
///
/// 真卡死没法按需复现，故用 `CURSEEN_REC_WEDGE_AFTER=N` 注入。钉三件事：
/// 1. 产物有 `moov` + H264 轨（= 抢救生效，前段能播）；
/// 2. 结论报 `bytes = Some`（界面会走"已保存卡住前的部分"那条）；
/// 3. 结论**不能**是 `is_clean()` —— 卡死抢救出来的绝不能说成"录制完成"。
#[test]
#[ignore]
fn wedged_record_still_yields_a_playable_mp4() {
    init();
    clear_fault_injection();
    let out = std::env::temp_dir().join("curseen_wedge_smoke.mp4");
    let _ = std::fs::remove_file(&out);
    // 走到第 11 帧起就永久停住（`maybe_wedge` 的 `tick >= 10`），停在**采集**阶段，
    // 与现场一致。
    // ★ 现行变量是 `CURSEEN_REC_WEDGE_AT`（**阶段名**）。`..._AFTER`（帧号）是新版
    //   分段方案留下的旧变量、已随新版一起弃用：留着它等于"根本没楔住"，这条用例
    //   会退化成一段普通录制（结论 `error=None`），反而把"不能报成完成"那条断言打红
    //   —— 也就是说它一直在测一个**不存在**的故障。
    std::env::set_var("CURSEEN_REC_WEDGE_AT", "acquire");

    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 0,
        hardware: recorder::Hardware::Auto,
        show_cursor: false,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");
    let _ = wait_first_frame(&rec);
    // 等它真的楔住（多给它一倍时间，别让"还没卡"被当成"没卡"）。
    std::thread::sleep(Duration::from_secs(6));
    let wedged_frames = rec.frames();

    let (tx, rx) = std::sync::mpsc::channel();
    // 只等 3 秒就弃子 —— 卡死的线程是等不回来的，这正是要走抢救分支的路。
    rec.finish_detached(Duration::from_secs(3), move |o| {
        let _ = tx.send(o);
    });
    let done = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("收尾线程应交回结论");
    std::env::remove_var("CURSEEN_REC_WEDGE_AT");

    let head = std::fs::read(&out).expect("应能读回产物");
    println!(
        "卡死抢救：{wedged_frames} 帧 / {} 字节，结论 {:?}",
        done.bytes.unwrap_or(0),
        done.error
    );
    // ★ 注入点就在**第 10 帧**（`maybe_wedge` 的 `tick >= 10`），所以卡住时帧数必然
    //   冻在 10 —— 断言写 ">几十帧" 是拿"注入点在第 90 帧"的旧变量留下的期望，
    //   现在那个变量已经不存在了。
    assert!(
        wedged_frames >= 10,
        "卡死前该录到注入点（第 10 帧）以上，实际 {wedged_frames}"
    );
    // ★ 这条能成立靠的是**编码侧自救**（`encode_worker` 的 `ENC_IDLE_GRACE`）：
    //   采集线程卡死时它既不关通道也不送帧，于是 `moov` 只能由**还活着的**编码线程
    //   来写。没有那条逻辑，采集侧楔住 = 产物永远缺索引 = 内容全丢。
    assert!(
        head.windows(4).any(|w| w == b"moov"),
        "卡死时也必须补上 moov —— 少了它几百 MB 都是播不了的废文件（编码侧自救没生效？）"
    );
    assert!(
        head.windows(4).any(|w| w == b"avc1"),
        "缺少 H264 (avc1) 编码轨"
    );
    assert!(
        done.bytes.is_some(),
        "抢救成功就该报有产物，实际 {:?}",
        done.error
    );
    assert!(
        !done.is_clean(),
        "卡死抢救出来的不能报成「完整录完」，界面会把中断说成完成"
    );
}

/// ★ 长录制不楔住（2026-10-02 现场复现用例）。
///
/// 现场：录到 223 秒时录制线程突然停摆 —— 不报错、不写帧、`stop` 也唤不醒，
/// 13 分钟后用户按停止，`finish_detached` 等 30 秒只能弃子，留下 177MB 无 `moov`
/// 的废文件。短录制（≤46s）从来没复现过，所以这条**必须真的录够久**。
///
/// 钉两件事：
/// 1. 整个录制期心跳不停摆（停摆即报出**卡在哪一步**，见 `RecStage`）；
/// 2. 结束时能正常 `Finalize`（有 `moov`）。
///
/// 时长用 `CURSEEN_LONG_REC_S` 调（默认 300 秒）。跑：
/// ```text
/// cargo test --release --test capture_smoke long_record -- --ignored --nocapture --test-threads=1
/// ```
#[test]
#[ignore]
fn long_record_never_wedges() {
    init();
    clear_fault_injection();
    let secs: u64 = std::env::var("CURSEEN_LONG_REC_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let out = std::env::temp_dir().join("curseen_smoke_long.mp4");
    let _ = std::fs::remove_file(&out);
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(8000),
        max_long_edge: 0,
        // ★ 现场就是"自动 → 硬件编码"，复现必须同配置。
        hardware: recorder::Hardware::Auto,
        show_cursor: true,
        capture_mic: true,
        capture_speaker: true,
        mic_denoise: "medium".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");
    let _ = wait_first_frame(&rec);

    // 边录边盯心跳：停摆 15 秒就带着"卡在哪一步"的红字退出。
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let mut last_report = 0u64;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1_000));
        let stall = rec.stall_ms();
        let (stage, tick) = rec.progress();
        if stall >= 15_000 {
            panic!(
                "录制心跳停摆 {:.0}s：卡在「{}」（第 {tick} 帧，已录 {} 帧）",
                stall as f64 / 1000.0,
                stage.name(),
                rec.frames()
            );
        }
        let frames = rec.frames();
        if frames - last_report >= 300 {
            last_report = frames;
            println!("  …已录 {frames} 帧（阶段「{}」）", stage.name());
        }
    }
    let frames = rec.frames();

    let (tx, rx) = std::sync::mpsc::channel();
    rec.finish_detached(Duration::from_secs(30), move |o| {
        let _ = tx.send(o);
    });
    let done = rx
        .recv_timeout(Duration::from_secs(35))
        .expect("收尾线程应交回结论");
    println!("长录 {secs}s / {frames} 帧 -> {:?}", done);
    assert!(
        done.is_clean(),
        "长录制应能正常收尾，实际: {:?}",
        done.error
    );
    let bytes = std::fs::read(&out).expect("产物应存在");
    assert!(
        bytes.windows(4).any(|w| w == b"moov"),
        "长录制产物缺 moov —— Finalize 没跑到"
    );
    println!("产物 {} 字节，含 moov", bytes.len());
}

/// 等到录屏线程真的出了第一帧（顺带测出启动开销），返回那一刻。
fn wait_first_frame(rec: &recorder::Recorder) -> std::time::Instant {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if rec.frames() > 0 {
            return std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("10 秒内没录到第一帧：{:?}", rec.take_error());
}

/// 从 MP4 的 `mvhd` box 里读时长（秒）。只认 32 位 version 0 的 mvhd。
fn mp4_seconds(bytes: &[u8]) -> Option<f64> {
    let at = bytes.windows(4).position(|w| w == b"mvhd")?;
    // mvhd: [version(1) + flags(3)] [ctime 4] [mtime 4] [timescale 4] [duration 4]
    let p = at + 4 + 4 + 4 + 4;
    let timescale = u32::from_be_bytes(bytes[p..p + 4].try_into().ok()?) as f64;
    let duration = u32::from_be_bytes(bytes[p + 4..p + 8].try_into().ok()?) as f64;
    if timescale <= 0.0 {
        return None;
    }
    Some(duration / timescale)
}

/// ★★ 卡在 `AcquireNextFrame` 里**还能不能被叫醒**（根治手段可行性验证）：
/// 上一条证明"旧会话不释放就建不了新会话"，而释放只能靠卡住的那个线程自己走回来。
/// 社区常见的解法是从别的线程 `ReleaseFrame()` 把阻塞调用打断。这条用例就是问：
/// **本机这版驱动吃不吃这一套**？吃 → 卡死可以自愈；不吃 → 只能靠架构绕开。
#[test]
#[ignore]
fn release_frame_from_another_thread_wakes_a_blocked_acquire() {
    init();
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::*;

    // DXGI 对象是 free-threaded，跨线程调用本身合法；windows-rs 的包装不 Send，
    // 这里手工标注（与 `recorder.rs` 里 `SendSink` 同一套路）。
    struct SendDup(IDXGIOutputDuplication);
    unsafe impl Send for SendDup {}
    unsafe impl Sync for SendDup {}

    unsafe {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .expect("建 D3D11 设备");
        let device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast().expect("IDXGIDevice");
        let adapter = dxgi.GetAdapter().expect("GetAdapter");
        let out: IDXGIOutput = adapter.EnumOutputs(0).expect("EnumOutputs");
        let o1: IDXGIOutput1 = out.cast().expect("IDXGIOutput1");
        let dup = Arc::new(SendDup(
            o1.DuplicateOutput(&device).expect("DuplicateOutput"),
        ));

        let returned = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<()>();

        // 线程 A：无限等待地取帧（桌面静止 ⇒ 一直不返回，正好复现"卡住"）。
        let d = dup.clone();
        let r = returned.clone();
        std::thread::spawn(move || {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            let _ = d.0.AcquireNextFrame(0xFFFFFFFF, &mut info, &mut res); // INFINITE
            r.store(true, Ordering::SeqCst);
            let _ = tx.send(());
        });

        std::thread::sleep(Duration::from_millis(1500));
        println!("[A] 1.5s 后仍阻塞: {}", !returned.load(Ordering::SeqCst));

        // 线程 B：从别的线程 ReleaseFrame，试图把 A 叫醒。
        let d2 = dup.clone();
        let kick = std::thread::spawn(move || d2.0.ReleaseFrame());
        let kick_err = kick.join();
        println!("[B] 别的线程 ReleaseFrame: {:?}", kick_err);

        let woke = rx.recv_timeout(Duration::from_secs(5)).is_ok();
        println!("[C] 5 秒内 A 被叫醒: {}  ← 这就是结论", woke);
    }
}

/// ★★ 失效重建到底靠什么才能成：**释放旧的**（DXGI 同一时刻只容一份复制对象）。
///
/// 这条用例定的是"卡死 / `ACCESS_LOST` 之后还有没有别的自愈路子"，结论是**没有**：
/// 换一台全新的 D3D 设备照样 `0x80070057`。所以唯一的路就是"先释放、再新建"
/// （`recorder.rs` 里 `DuplHolder::replace` 做的事），别再设想"卡死了换设备重连"。
///
/// 与 `rebuilding_a_duplication_needs_the_old_one_gone_first` 的分工：那条只钉
/// "同设备上先放后建能成"，本条钉的是**换设备也不成**（防止有人把自愈方案改成换设备）。
#[test]
#[ignore]
fn a_second_duplication_needs_the_old_one_released() {
    init();
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::*;

    /// 建一台独立的 D3D11 设备（与 `recorder.rs` 里录制用的同款参数）。
    unsafe fn fresh_device() -> (ID3D11Device, Option<ID3D11DeviceContext>) {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .expect("建 D3D11 设备");
        (device.unwrap(), context)
    }

    /// 某台设备的第 0 块输出。
    unsafe fn first_output1(d: &ID3D11Device) -> IDXGIOutput1 {
        let dxgi: IDXGIDevice = d.cast().expect("ID3D11Device → IDXGIDevice");
        let adapter = dxgi.GetAdapter().expect("GetAdapter");
        let out: IDXGIOutput = adapter.EnumOutputs(0).expect("EnumOutputs");
        out.cast().expect("IDXGIOutput → IDXGIOutput1")
    }

    unsafe {
        let (d1, _c1) = fresh_device();
        let o1 = first_output1(&d1);
        let dup1 = o1.DuplicateOutput(&d1);
        println!("[1] 第一个会话        : ok={}", dup1.is_ok());
        assert!(
            dup1.is_ok(),
            "第一个会话必须能建起来，否则本机不适合跑本探针"
        );

        let dup_same = o1.DuplicateOutput(&d1);
        println!(
            "[2] 同一设备上再建一个: ok={} err={:?}",
            dup_same.is_ok(),
            dup_same.as_ref().err().map(|e| e.code())
        );
        assert!(
            dup_same.is_err(),
            "旧的还活着就建出了第二个 —— 那并发限制不存在，rebuild 的顺序约束该重审"
        );

        let (d2, _c2) = fresh_device();
        let o2 = first_output1(&d2);
        let dup_new = o2.DuplicateOutput(&d2);
        println!(
            "[3] 换台新设备再建一个: ok={} err={:?}",
            dup_new.is_ok(),
            dup_new.as_ref().err().map(|e| e.code())
        );
        // ★★★ 本条用例存在的理由：换设备**救不了**。限制是"同一时刻全进程只容一份
        //   桌面复制"，不是"每台设备一份"。所以"卡死了换台设备重连采集"这条路不存在，
        //   唯一的自愈是先释放旧的（见步骤 4 与 `DuplHolder::replace`）。
        assert!(
            dup_new.is_err(),
            "换设备竟然建成了 —— 那'换设备自愈'是条路，值得改走它（并重审本用例的结论）"
        );

        // ★ 决定性实验：把第一个复制对象**释放掉**之后，同一台设备上还能不能建？
        //   能建 ⇒ `recorder.rs` 里 `DXGI_ERROR_ACCESS_LOST` 的重建顺序（先建新的、成功后
        //         替换旧的）就是**确凿的 bug**：旧对象还活着时 `DuplicateOutput` 必然
        //         0x80070057，于是"桌面切换 / 模式变化"这种天天发生的事件 = 录制直接报错终止。
        //   不能建 ⇒ 对象被内核长期持有，释放无用，卡死后只能等或换输出。
        let _ = dup1.as_ref().unwrap().ReleaseFrame();
        drop(dup1);
        let dup_after_drop = o1.DuplicateOutput(&d1);
        println!(
            "[4] 释放第一个会话后再建: ok={} err={:?}",
            dup_after_drop.is_ok(),
            dup_after_drop.as_ref().err().map(|e| e.code())
        );
        assert!(
            dup_after_drop.is_ok(),
            "释放旧的之后仍建不起来（err={:?}）—— 那重建策略整个不成立，\
             '先放后建' 之外还有别的东西在占着，得另找根因",
            dup_after_drop.as_ref().err().map(|e| e.code())
        );

        // [5] 顺带测：等待 500ms（有些实现是异步回收）后再试，换设备。
        //   留着是为了盯住"是不是异步回收延迟"——若哪天这里变成 ok，说明驱动改成异步
        //   回收了，`replace` 里就不必（非） sleep，重跑一遍本用例再改结论。
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (d3, _c3) = fresh_device();
        let o3 = first_output1(&d3);
        let dup_after_wait = o3.DuplicateOutput(&d3);
        println!(
            "[5] 等 500ms 且换设备后再建: ok={} err={:?}",
            dup_after_wait.is_ok(),
            dup_after_wait.as_ref().err().map(|e| e.code())
        );
    }
}
