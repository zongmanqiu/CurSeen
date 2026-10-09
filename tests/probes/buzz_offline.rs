//! # 降噪音乐噪声取证探针（`buzz_offline`）—— 2026-10-09
//!
//! 上一轮 `buzz_ab`（真录 A/B）被环境噪声不可控污染。本探针做**完全受控**实验：
//! 采集同一段原始麦克风 PCM（denoise=off 直通），落盘 `raw.pcm`；再对**同一份
//! 输入**离线跑 `denoise_offline(.., "weak")`，落盘 `weak.pcm`。Python 对比两份
//! 输出的底噪谱——任何差异都 100% 归因于 RNNoise 本身。
//!
//! 跑法：`python build/probe.py run buzz_offline`（真录 6s）。

use std::time::{Duration, Instant};

use curseen_ui::platform::audio_capture::{denoise_offline, AudioCapture};

#[test]
#[ignore = "真录 6s；跑法见文件头"]
fn buzz_offline() {
    let ac = AudioCapture::start(true, false, "off", "", "").expect("起采集失败");
    let t0 = Instant::now();
    let mut raw: Vec<i16> = Vec::new();
    while t0.elapsed() < Duration::from_secs(6) {
        raw.extend(ac.take_samples());
        std::thread::sleep(Duration::from_millis(5));
    }
    ac.stop();
    drop(ac);
    println!("原始 PCM {} 个 i16 ≈ {:.2}s", raw.len(), raw.len() as f64 / 96000.0);
    assert!(raw.len() > 96000 * 4, "采到的样本太少");

    write_pcm("../temp/buzz_raw.pcm", &raw);
    let weak = denoise_offline(&raw, 2, "weak");
    assert_eq!(weak.len(), raw.len(), "weak 输出长度应等于输入（透传长度不变）");
    write_pcm("../temp/buzz_weak_pcm.pcm", &weak);
    println!("产物: ../temp/buzz_raw.pcm（原始）/ ../temp/buzz_weak_pcm.pcm（同一输入过 weak）");
}

fn write_pcm(path: &str, samples: &[i16]) {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, &bytes).expect("写 PCM 失败");
}
