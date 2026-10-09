//! # 全面联动真机测试（`soak_link`）
//!
//! ## 它要锁什么
//!
//! 录制失败类问题的根因（编码器节流反压楔住 / 空音频流 Finalize 卡死 /
//! UI 无限 join）修掉之后，剩下的风险是「**联动**」：真实使用时录屏从来不是
//! 单独跑的，拖尾、悬浮标、点击波纹、击键回显、虚拟键盘、按键音效各有自己的
//! Lua 引擎 / WASAPI / 渲染通路，多条管线并发才是冲突最容易暴露的场合。
//!
//! ## 本例测什么（"全面联动" = 下面这些同时跑）
//!
//! | 联动项 | 怎么跑的 | 为什么它参与 |
//! |---|---|---|
//! | 录屏 MP4（硬编） | 真 `Recorder`（生产链路，`finish_detached` 收尾） | 被测主体 |
//! | 鼠标拖尾 ×3 脚本 | 真 `FxCore`：`glow`/`ribbon`/`fireflies` | 三条独立 Lua 管线 |
//! | 悬浮标 | 真 `FxCore`：`solid` | 走 `set_cursor` 路径 |
//! | 点击波纹 | 真 `FxCore::press` + `disc` | 走 `press` 而非 `move_to` |
//! | 复合脚本 | `CompanionHost` + `3.composite_demo` | 按键分发通路（`on_key`） |
//! | 击键声音 | `CompanionHost::on_key` + `class_sound` | 声音走 WASAPI，与 MF 音频同批 |
//! | 虚拟键盘 | 真 `VirtualKeyboardModule`（`note_press` + `draw`） | 键帽高亮渲染通路 |
//! | 鼠标按键分发 | `CompanionHost::on_button` | 点击类脚本的另一条入口 |
//! | 系统声音 | `Recorder` 的 `capture_speaker`（loopback） | 音频通路也进 MF |
//! | 音画同步标记 | 外部驱动：参考音 + 屏幕白闪（`SOAK_LINK_AV_MARKER=1`） | 录完后做音画同步分析 |
//!
//! 全部**真跑**（真解释 Lua、真写 `DrawList`、真开音频采集），不是 sleep 占位。
//!
//! ## 判据（挑"出问题的方式"，不是挑"跑多少次"）
//!
//! ① 录制心跳停摆 > 60 秒 ⇒ 死锁。
//! ② 任一 `FxCore` / companion 在宽限期内一帧都不画 ⇒ 那条管线被拖死 / Lua 卡住。
//! ③ 录制帧数**倒退** ⇒ 采集侧状态错乱。
//! ④ 产物缺 `moov` / 时长与墙钟差 > 5% ⇒ 收尾没跑到（老症状）。
//! ⑤ 实测帧率 < 20fps（设定 30）⇒ 联动把管线拖垮了。
//!
//! ## 跑法（**不进日常编译**，见 `Cargo.toml` 探针说明）
//!
//! ```text
//! python build/probe.py run soak_link                    # 默认 300 秒
//! SOAK_LINK_SECS=7200 python build/probe.py run soak_link  # 120 分钟长录
//! SOAK_LINK_AV_MARKER=1 ...                              # 同时起参考音+白闪驱动
//! ```
//!
//! ★ 必须带 `--test-threads=1`（`probe.py` 已带）：DXGI 桌面复制全进程只容一份。

#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use curseen_ui::core::config::Config;
use curseen_ui::core::draw::DrawList;
use curseen_ui::core::events::KeyCode;
use curseen_ui::core::geometry::ScreenRect;
use curseen_ui::modules::trail::companion::CompanionHost;
use curseen_ui::modules::trail::fx_core::FxCore;
use curseen_ui::modules::virtual_keyboard::VirtualKeyboardModule;
use curseen_ui::modules::Module;
use curseen_ui::platform::recorder::{
    Hardware, RecKind, RecOptions, RecStage, RecordArea, Recorder,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// 心跳停摆多久判死锁。2026-10-05 实测那次是 60 秒不恢复。
const STALL_DEAD_MS: u64 = 60_000;

/// 探针内置日志器：把录制内核里的 `log::info!/warn!` 打到 stdout（cargo
/// `--nocapture` 下可见）。生产里日志走全局配置，探针进程没配就是无声的。
struct PrintLogger;
impl log::Log for PrintLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            println!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

/// 一条特效管线。
struct Fx {
    core: FxCore,
    /// 累计"产出了绘制指令"的帧数 —— 判据②用它。
    drawn: u64,
    name: &'static str,
}

#[test]
#[ignore = "全面联动真机测试：真起桌面复制 + 多条 Lua 管线 + 音频；跑法见文件头"]
fn everything_runs_together_without_wedging() {
    let _ = log::set_boxed_logger(Box::new(PrintLogger));
    log::set_max_level(log::LevelFilter::Info);
    let secs: u64 = std::env::var("SOAK_LINK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    // 收尾等待上限（秒）。2026-10-06 实测 7200s 长录在 90s 内没收完，
    // 需要拉长观察"Finalize 是慢还是死"——用 SOAK_FINISH_TIMEOUT_SECS 调。
    let finish_secs: u64 = std::env::var("SOAK_FINISH_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);
    if secs == 0 {
        // SOAK_LINK_SECS=0 = 跳过录制，只跑 `parser_reads_known_products`
        // （解析器自证用例）。probe.py 会把本 target 里所有 #[ignore] 都跑一遍，
        // 录制用例没有"只跑其一"的开关，用 0 当哨兵。
        println!("SOAK_LINK_SECS=0：跳过录制，只跑解析器自证用例");
        return;
    }
    // 音画同步标记驱动（参考音 + 屏幕白闪）：录完交给 `av_sync` 探针分析。
    let av_marker: bool = std::env::var("SOAK_LINK_AV_MARKER")
        .map(|v| v == "1")
        .unwrap_or(false);

    let lua_root = PathBuf::from("../build/assets/lua");
    assert!(
        lua_root.join("MouseTrail").is_dir(),
        "找不到 {:?} —— 先跑 deploy.py 把素材同步过去",
        lua_root
    );

    println!("════ 全面联动测试：{secs} 秒 ════");
    println!("  录制 = MP4 硬编 + 系统声音；特效 = 5 条真 FxCore + 虚拟键盘 + companion（按键+声音）");
    if av_marker {
        println!("  音画标记驱动：开（参考音 440Hz + 每 5s 1760Hz 标记 + 屏幕白闪）");
    }

    // ── 1. 起录（含系统声音：音频通路同样走 MF，必须纳入联动）──
    let out_dir = std::path::PathBuf::from("../temp/soaklink");
    let _ = std::fs::create_dir_all(&out_dir);
    let out = out_dir.join(format!("soak_link_{secs}s.mp4"));
    let _ = std::fs::remove_file(&out);

    let rec = Recorder::start(RecOptions {
        kind: RecKind::Mp4,
        path: out.clone(),
        area: RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(8_000),
        max_long_edge: 0,
        hardware: Hardware::Auto,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: true, // 系统声音（loopback）：不需额外权限
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");

    let mut waited = 0u32;
    while rec.frames() == 0 && waited < 300 {
        std::thread::sleep(Duration::from_millis(100));
        waited += 1;
    }
    assert!(
        rec.frames() > 0,
        "300ms 内没有首帧（起录失败）
  录制线程错误: {:?}",
        rec.take_error()
    );
    println!("  起录成功（等首帧 {}ms）", waited * 100);

    // ── 1b. 音画标记驱动：起录成功后立刻拉起（参考音与白闪同钟起步）──
    let mut driver: Option<std::process::Child> = None;
    if av_marker {
        match std::process::Command::new("python")
            // ★ cwd = main/（cargo test 的包根目录）：build/ 在 main 下，不是仓库根。
            .arg("build/av_marker_driver.py")
            .env("MARKER_SECS", (secs + 120).to_string())
            .spawn()
        {
            Ok(c) => {
                println!("  音画标记驱动已启动（pid {}，{}s 后自退）", c.id(), secs + 120);
                driver = Some(c);
            }
            Err(e) => panic!("音画标记驱动拉不起来（python ../build/av_marker_driver.py）: {e}"),
        }
    }

    // ── 2. 起全部特效管线（真 Lua 引擎）──
    let (w, h) = unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN).max(1920) as f32,
            GetSystemMetrics(SM_CYSCREEN).max(1080) as f32,
        )
    };
    let mut fxs: Vec<Fx> = Vec::new();
    // (目录, 脚本 id, 显示名, 宿主)
    for (dir, id, name, host) in [
        ("MouseTrail", "glow", "拖尾·流光", "trail"),
        ("MouseTrail", "ribbon", "拖尾·飘带", "trail"),
        ("MouseTrail", "fireflies", "拖尾·萤火", "trail"),
        ("HoverDot", "solid", "悬浮标", "HoverDot"),
        ("ClickFx", "disc", "点击波纹", "ClickFx"),
    ] {
        match FxCore::load(&lua_root.join(dir), id, host) {
            Ok(mut core) => {
                core.set_screen(w, h);
                fxs.push(Fx {
                    core,
                    drawn: 0,
                    name,
                });
                println!("  · {name}（{dir}/{id}.lua）已加载");
            }
            Err(e) => {
                // ★ 缺素材不静默跳过：那就等于"没测联动"，必须报出来。
                panic!("{name} 特效加载失败（{dir}/{id}.lua）: {e}");
            }
        }
    }
    assert_eq!(fxs.len(), 5, "五条 FxCore 管线都要起来");

    // ── 2b. companion：按键回显 + 击键声音（`on_key` 通路）──
    let mut companion = CompanionHost::new();
    let mut params = HashMap::new();
    params.insert("volume".to_string(), 0.5.into());
    companion.sync_at(
        &lua_root.join("scripts"),
        &[
            ("3.composite_demo".to_string(), params.clone()),
            ("class_sound".to_string(), params),
        ],
    );
    println!("  · 复合脚本 + 击键声音（scripts/3.composite_demo + class_sound）已挂");

    // ── 2c. 虚拟键盘：真模块真渲染（`note_press` + `Module::draw`）──
    let mut cfg = Config::default();
    cfg.virtual_keyboard.enabled = true;
    cfg.virtual_keyboard.layout = "wasd_gaming".to_string();
    let mut vk = VirtualKeyboardModule::new(&cfg);
    vk.set_screen(ScreenRect::new(0, 0, w as i32, h as i32));
    // 依次点亮这些键帽（按下 → 下一拍抬起，键帽高亮走 hold+淡出通路）。
    let vk_keys = [
        KeyCode::Char('W'),
        KeyCode::Char('A'),
        KeyCode::Char('S'),
        KeyCode::Char('D'),
        KeyCode::Space,
        KeyCode::LShift,
        KeyCode::MouseLeft,
    ];
    let mut vk_presses = 0u64;
    println!("  · 虚拟键盘（wasd_gaming，7 键轮替点亮）已挂");

    // ── 3. 主循环：录制 + 全部管线同时推进 ──
    let t0 = Instant::now();
    let mut next_report = 10u64;
    let mut last_frames = rec.frames();
    let mut list = DrawList::new();
    let mut frame = 0u64;
    // 依次敲这些键，让 companion 的 on_key 真被喂到（→画 + 出声）
    let keys = ["A", "S", "D", "W", "Space", "Shift", "Ctrl", "Alt"];

    while t0.elapsed().as_secs() < secs {
        let t = frame as f32 / 60.0;
        let x = w * 0.5 + w * 0.28 * (t * 0.7).sin();
        let y = h * 0.5 + h * 0.24 * (t * 1.1).sin();

        // 3a. 五条 FxCore 管线各推一帧（真 advance + 真 render）
        for fx in fxs.iter_mut() {
            fx.core.set_elapsed(t);
            fx.core.set_wall_ms(t0.elapsed().as_secs_f64() * 1000.0);
            fx.core.move_to(x, y, None);
            fx.core.set_cursor(x, y);
            fx.core.advance(1.0 / 60.0);
            list.clear();
            if fx.core.render(&mut list).produced {
                fx.drawn += 1;
            }
        }

        // 3b. 点击波纹走 press 路径、companion 走 on_button（每 120 帧一次）
        if frame % 120 == 60 {
            for fx in fxs.iter_mut() {
                if fx.name == "点击波纹" {
                    fx.core.press(x, y);
                }
            }
            companion.on_button("left", x, y, t, (w, h));
        }

        // 3c. companion：每 24 帧敲一个键（on_key → 出声 + 画击键回显）
        if frame % 24 == 0 {
            let k = keys[(frame / 24) as usize % keys.len()];
            companion.on_key(k, (frame % 96 == 0, false, false, false), false, t, (w, h));
        }
        // companion 每帧也画（续帧动画）
        list.clear();
        companion.draw(&mut list, t, (w, h));

        // 3d. 虚拟键盘：每 30 帧按下一颗键、再过 15 帧抬起（键帽高亮+淡出）
        if frame % 30 == 0 {
            let k = vk_keys[(frame / 30) as usize % vk_keys.len()];
            vk.note_press(k, true);
            vk_presses += 1;
        } else if frame % 30 == 15 {
            let k = vk_keys[((frame - 15) / 30) as usize % vk_keys.len()];
            vk.note_press(k, false);
        }
        list.clear();
        vk.draw(&mut list, &cfg);

        // ── 判据①：录制心跳停摆 ⇒ 死锁 ──
        let stall = rec.stall_ms();
        let (cur_stage, _) = rec.progress();
        // ★ '已结束' 阶段 = 录制线程自己退出了（出错收尾），**不是死锁**。
        //   心跳停摆是它的**后果**不是原因；混为一谈会把录制提前结束误报成
        //   录制死锁，指向完全不同的排查方向（§12.6）。
        if cur_stage != RecStage::Done && stall >= STALL_DEAD_MS && rec.frames() > 0 {
            let (stage, tick) = rec.progress();
            panic!(
                "① 录制线程死锁：第 {}s 心跳停摆 {}ms（卡在「{}」第 {tick} 帧）",
                t0.elapsed().as_secs(),
                stall,
                stage.name()
            );
        }
        // ── 判据③：帧数倒退 = 状态错乱 ──
        if rec.frames() < last_frames {
            panic!("③ 录制帧数倒退：{} < {}", rec.frames(), last_frames);
        }
        // ── 判据②：特效 20 秒后仍一帧不画 ⇒ 那条管线死了 ──
        if frame % 1800 == 1799 {
            let el = t0.elapsed().as_secs();
            if el > 20 {
                for fx in fxs.iter() {
                    if fx.drawn == 0 {
                        panic!(
                            "② 特效「{}」在 {el:?} 内一帧都没画（联动把它拖死了，或 Lua 没跑）",
                            fx.name
                        );
                    }
                }
            }
        }

        let el = t0.elapsed().as_secs();
        if el >= next_report {
            let (stage, tick) = rec.progress();
            let drawn: Vec<String> = fxs
                .iter()
                .map(|f| format!("{}={}", f.name, f.drawn))
                .collect();
            println!(
                "  … {el:>4}s 录制帧 {tick}｜停摆 {stall}ms｜阶段 {}｜击键 {vk_presses}｜{}",
                stage.name(),
                drawn.join(" ")
            );
            next_report += 10;
        }

        last_frames = rec.frames();
        frame += 1;
        let due = t0 + Duration::from_nanos(frame * 33_333_333);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        } else {
            frame = 0; // 落后太多就重新对表，不追补
        }
    }

    // ── 4. 收尾 + 产物校验 ──
    let frames = rec.frames();
    let written = rec.written();
    let dropped = rec.dropped();
    let wall = t0.elapsed().as_secs_f64();
    let (tx, rx) = std::sync::mpsc::channel();
    let t_fin = Instant::now();
    rec.finish_detached(Duration::from_secs(finish_secs), move |d| {
        let _ = tx.send((d, t_fin.elapsed().as_secs_f64()));
    });
    // 回调在"限时 join 返回"后立刻发：正常退出时 fin_lat = 真实收尾耗时
    // （停采 + 音尾 + Finalize）；超时放弃时 fin_lat ≈ finish_secs 且
    // `done.error` 会带停在哪个阶段 —— 两种情况都能分辨。
    let (done, fin_lat) = rx
        .recv_timeout(Duration::from_secs(finish_secs + 10))
        .expect("收尾回调超时（录制线程可能又楔住了）");
    println!("  收尾：停止位→线程退出耗时 {fin_lat:.1}s（上限 {finish_secs}s）");

    // 驱动用完即杀（正常它会自退，这里兜底别留后台进程）。
    if let Some(c) = driver.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);

    println!("\n{secs}s 段：录制帧 {frames}｜墙钟 {wall:.1}s｜产物 {size}B");
    for fx in fxs.iter() {
        println!("  特效 {}：活动帧 {}", fx.name, fx.drawn);
    }
    println!("  虚拟键盘：键帽点亮 {vk_presses} 次");
    if let Some(e) = &done.error {
        panic!("④ 收尾报错: {e}");
    }
    assert!(size > 0, "④ 产物 0 字节");

    // ★★ ⑥ 采集/编码解耦（2026-10-06）之后多出来的失败形态：**采集在跑、文件不长**。
    //   以前"帧数在涨"就等于"在录"，现在不够了 —— 编码器跟不上时采集侧会照常出帧、
    //   把帧丢掉。丢帧率就是这条新路径上唯一的现场指标。
    let drop_rate = dropped as f64 / frames.max(1) as f64;
    println!(
        "  采集 {frames} / 写入 {written} / 丢弃 {dropped}（{:.2}%）",
        drop_rate * 100.0
    );
    assert!(
        drop_rate < 0.02,
        "⑥ 丢弃 {dropped}/{frames} 帧（{:.1}%）：编码跟不上采集（超过 2% 就是性能回归）",
        drop_rate * 100.0
    );
    assert!(
        written > 0 && written <= frames,
        "⑥ 写入帧数 {written} 与采集帧数 {frames} 不自洽（写入应 >0 且 ≤ 采集）"
    );

    let boxes = mp4_top_boxes(&out);
    println!("  顶层盒: {boxes:?}");
    assert!(
        boxes.iter().any(|b| b == "moov"),
        "④ 产物缺 moov = 收尾没跑到，任何播放器都打不开"
    );

    // ★★ 视频轨样本数：`mvhd` 取所有轨最大值，**音频正常时会掩盖视频被截断**
    //   （实测撞到视频 100 样本/3.33s、音频 600.17s，`mvhd` 报 600s 断言 PASS）。
    let vsamples = mp4_video_sample_count(&out);
    if let Some(v) = vsamples {
        let expect = (wall * 30.0).round() as u64;
        println!(
            "  视频轨样本：{v}（按 30fps 应约 {expect}，差 {:.1}%）",
            (v as f64 - expect as f64).abs() * 100.0 / expect.max(1) as f64
        );
    } else {
        println!("  视频轨样本：★解析不到（`moov` 里没找到视频轨的 stsz）");
    }
    // ★★ 先判视频轨样本数，再判总时长 —— 顺序反了会被音频轨掩盖（见上面注释）。
    if let Some(v) = mp4_video_sample_count(&out) {
        let expect = (wall * 30.0).round() as u64;
        // 容忍 2%：个别帧被丢是正常的（时长会由墙钟间隔补偿）。
        let floor = expect.saturating_sub(expect / 50 + 10);
        assert!(
            v >= floor,
            "★ 视频轨只有 {v} 个样本（应约 {expect}）—— 视频被截断了，\
             而 `mvhd` 总时长看不出来（音频轨会把它顶上去）"
        );
    }
    if let Some(mvhd) = mp4_mvhd_seconds(&out) {
        let rel = (mvhd - wall).abs() / wall;
        println!(
            "  容器时长: {mvhd:.2}s（墙钟 {wall:.1}s，差 {:.2}%）",
            rel * 100.0
        );
        assert!(
            rel < 0.05,
            "④ 容器时长与墙钟差 {:.1}%（>5%，尾段被截断）",
            rel * 100.0
        );
    }

    let fps = frames as f64 / wall;
    println!("  实际帧率: {fps:.1}fps（设定 30）");
    assert!(fps > 20.0, "⑤ 帧率只 {fps:.1}fps，联动把管线拖垮了");

    println!("\n✅ 全面联动 {secs}s：录制无死锁、七条管线都活着、产物完整可播");
}

/// 顶层盒索引：seek 流式扫描（2h 产物 4.9GB，**绝不整文件读进内存**）。
/// 返回 (类型名, 盒起始偏移, 盒总长)。
///
/// ★ 64 位盒长（`size==1` + 8 字节 largesize）：ISO BMFF 里 largesize 是
///   **含 16 字节头的整盒长**，`end = off + largesize`。曾写 `off + 16 + big`
///   把盒头重复计入 —— 走完 4.9GB 的 mdat 后越过 moov 头 16 字节读进垃圾，
///   2026-10-06 7200s 长测据此误报"缺 moov"（产物其实完整，`mp4_dur.py`
///   独立判定：7200.13s / 216004 视频样本 / 337505 音频样本全齐）。
///   只有 mdat > 4GB 才走 64 位分支 ⇒ 短程冒烟永远测不出这个 bug。
fn mp4_top_box_index(path: &std::path::Path) -> Vec<(String, u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut out = Vec::new();
    let mut off = 0u64;
    let mut hdr = [0u8; 16];
    while off + 8 <= len {
        if f.seek(SeekFrom::Start(off)).is_err() {
            break;
        }
        let got = f.read(&mut hdr).unwrap_or(0);
        if got < 8 {
            break;
        }
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let ty = String::from_utf8_lossy(&hdr[4..8]).into_owned();
        let total = if size32 == 1 {
            if got < 16 || off + 16 > len {
                break;
            }
            u64::from_be_bytes(hdr[8..16].try_into().unwrap())
        } else if size32 == 0 {
            len - off // 盒长 0 = 到文件尾
        } else {
            size32
        };
        if total == 0 {
            break;
        }
        out.push((ty, off, total));
        let end = off + total;
        if end > len {
            break;
        }
        off = end;
    }
    out
}

/// 扫 MP4 顶层盒（名字列表，判据用）。
fn mp4_top_boxes(path: &std::path::Path) -> Vec<String> {
    mp4_top_box_index(path)
        .into_iter()
        .map(|(t, _, _)| t)
        .collect()
}

/// 读出 **moov 盒**的内容：偏移由顶部盒索引走出（★ 绝不裸搜 `"moov"` 字节——
/// mdat 里是任意视频数据，1.6GB 里撞出假命中的概率约 1/3，2026-10-06 1800s
/// 产物上 `"mvhd"` 裸搜撞假命中、解析出 0.02s 垃圾时长）。
fn mp4_moov_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let (_, off, total) = mp4_top_box_index(path)
        .into_iter()
        .find(|(t, _, _)| t == "moov")?;
    // 2h 产物的 moov 也只有 ~4.3MB；保险上限防异常文件。
    if total == 0 || total > 256 * 1024 * 1024 {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; total as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 数**视频轨**的样本数（`stsz`）。
///
/// ★ `mvhd` 报的是"所有轨里最长的那条"，音频轨正常时会**掩盖**视频轨被截断
///   （2026-10-05 实测：视频 100 样本/3.33s、音频 600.17s，`mvhd` 报 600s、断言 PASS）。
///   所以视频完整性必须**单独数样本**。
///
/// 做法：在 moov 盒内（由顶部盒索引定位，见 [`mp4_moov_bytes`]）逐 `trak` 找
/// `hdlr` 写着 `vide` 的那条，取它 `stbl` 里的 `stsz` 样本数。
fn mp4_video_sample_count(path: &std::path::Path) -> Option<u64> {
    let d = mp4_moov_bytes(path)?;
    let m_end = d.len();
    let find = |needle: &[u8], from: usize| {
        d.get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    };
    // moov 内容里不会有样本数据，`trak`/`vide`/`stsz` 四字符在此范围内只会
    // 作为盒类型出现 ⇒ 逐trak窗口搜索是安全的。
    let mut off = 4usize; // 跳过 moov 盒头
    while let Some(trak) = find(b"trak", off) {
        let trak_end = find(b"trak", trak + 4).unwrap_or(m_end);
        if find(b"vide", trak).is_some_and(|h| h < trak_end) {
            // 视频轨 → 在它的 stbl 里找 stsz
            if let Some(s) = find(b"stsz", trak) {
                if s + 16 <= trak_end {
                    // stsz: size(4) type(4) version(1) flags(3) sample_size(4) sample_count(4)
                    return Some(
                        u32::from_be_bytes([d[s + 12], d[s + 13], d[s + 14], d[s + 15]]) as u64,
                    );
                }
            }
        }
        off = trak + 4;
    }
    None
}

/// `mvhd` 的电影时长（秒）。只在 moov 盒内找 `mvhd`（理由见 [`mp4_moov_bytes`]）。
fn mp4_mvhd_seconds(path: &std::path::Path) -> Option<f64> {
    let d = mp4_moov_bytes(path)?;
    let i = d.windows(4).position(|w| w == b"mvhd")?;
    let p = &d[i + 4..];
    if p.is_empty() {
        return None;
    }
    if p[0] == 1 {
        if p.len() < 32 {
            return None;
        }
        let ts = u32::from_be_bytes([p[16], p[17], p[18], p[19]]) as f64;
        let du =
            u64::from_be_bytes([p[20], p[21], p[22], p[23], p[24], p[25], p[26], p[27]]) as f64;
        (ts != 0.0).then(|| du / ts)
    } else {
        if p.len() < 20 {
            return None;
        }
        let ts = u32::from_be_bytes([p[12], p[13], p[14], p[15]]) as f64;
        let du = u32::from_be_bytes([p[16], p[17], p[18], p[19]]) as f64;
        (ts != 0.0).then(|| du / ts)
    }
}

/// 解析器自证：拿**已知健康**的产物验判据自己（判据纪律——先怀疑判据）。
///
/// 期望值 = 文件名里的秒数。这些产物全部实录生成；1800s/7200s 两份经
/// `mp4_dur.py` 独立判定健康（2026-10-06，分别 1800.03s / 7200.13s）。
/// 不吃满清单也行（产物可能被清理），但至少要吃到 2 份，否则自证无意义。
#[test]
#[ignore = "解析器自证：需要 temp/soaklink 下有实录产物；SOAK_LINK_SECS=0 跑 probe.py 时只跑本用例"]
fn parser_reads_known_products() {
    let dir = std::path::PathBuf::from("../temp/soaklink");
    let cases = [
        ("soak_link_120s.mp4", 120.0),
        ("soak_link_300s.mp4", 300.0),
        ("soak_link_600s.mp4", 600.0),
        ("soak_link_1800s.mp4", 1800.0),
        ("soak_link_7200s.mp4", 7200.0),
    ];
    let mut checked = 0usize;
    for (name, want) in cases {
        let p = dir.join(name);
        if !p.exists() {
            println!("  跳过（不存在）：{name}");
            continue;
        }
        checked += 1;
        let boxes = mp4_top_boxes(&p);
        assert!(
            boxes.iter().any(|b| b == "moov"),
            "{name}：顶层盒无 moov（{boxes:?}）"
        );
        let secs = mp4_mvhd_seconds(&p).unwrap_or_else(|| panic!("{name}：解析不到 mvhd"));
        let rel = (secs - want).abs() / want;
        println!("  {name}：mvhd {secs:.2}s（期望 {want:.0}s，差 {:.2}%）", rel * 100.0);
        assert!(rel < 0.05, "{name}：mvhd {secs:.2}s 与期望差 {:.1}%", rel * 100.0);
        let v = mp4_video_sample_count(&p).unwrap_or_else(|| panic!("{name}：解析不到视频样本数"));
        let expect = (want * 30.0).round() as u64;
        let vrel = (v as f64 - expect as f64).abs() / expect as f64;
        println!("  {name}：视频样本 {v}（期望约 {expect}，差 {:.2}%）", vrel * 100.0);
        assert!(vrel < 0.05, "{name}：视频样本 {v} 与期望差 {:.1}%", vrel * 100.0);
    }
    assert!(checked >= 2, "自证至少要吃到 2 份产物（现 {checked}）");
}
