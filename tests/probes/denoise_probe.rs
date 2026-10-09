//! probe-group: fast
//! 降噪「到底有没有真的执行、三档差在哪」的**离线**验收。
//!
//! 跑法（`--ignored --nocapture`，单线程）：
//! ```text
//! cargo test --release --test denoise_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! # 为什么必须有这条
//!
//! 面板上的「试听」是**实时**链：要麦克风、要有人说话、要一台播放设备，自动化用不了。
//! 而"降噪是否真的执行"只能靠量 —— 光看开关是绿的什么也证明不了。这里用
//! [`audio_capture::denoise_offline`]（与录制同一套 `MicProcessor`）造一段
//! "说话 / 静音 / 说话 / 静音 / 说话"的信号，**分频段**量几件事：
//!
//! 1. **静音段的底噪掉了多少 dB** —— 整个静音段取 RMS，RNNoise 该把稳态噪声压下去；
//! 2. **说话段里的高频底噪掉了多少 dB** —— ★ 分水岭。RNNoise 针对人声训练，说话时
//!    也在压背景噪声（其噪声底估计在有人声时照样更新），旧的噪声门（已删除）只在
//!    整帧安静时清零、说话时的嘶嘶声原样保留，所以这一项现在是 RNNoise"说话时也在降"
//!    的自我证明。
//!    （不能拿"说话段总电平"当指标：底噪比人声低很多，总电平几乎不动，
//!    那样量出来的是"人声有没有被削"，不是"底噪有没有下去"。）
//! 3. **人声谐波还剩多少** —— 只压噪声不削人声才算真降噪；只把音量拧小不算。
//!
//! 断言只锁**相对关系**（方向正确、可分辨），不锁具体数字：换台机器、换段素材，
//! 绝对值都会变，但"开档比关档干净、人声没被削没"不会。
//!
//! ★ 素材用**确定性宽带噪声**（LCG），不是纯单频音：RNNoise 是针对"人声 + 真实（彩色/
//! 宽带）噪声"训练的，对纯正弦这种训练分布外的信号几乎不动；而真实麦克风底噪正是宽带
//! 的。用宽带噪声才能量出 RNNoise 真实会做的事。

use curseen_ui::platform::audio_capture::denoise_offline;

const RATE: usize = 48_000;
/// 说话段的谐波成分（人声元音近似）。
const VOICE_HZ: [f32; 3] = [200.0, 400.0, 800.0];
/// 用来量"说话时高频底噪"的频率点：落在人声谐波之外、宽带噪声有能量处。
const HF_HZ: f32 = 6000.0;

/// 合成素材：返回 (样本, 每段 (起, 止, 是否说话))。
///
/// 说话段是 200/400/800 Hz 的谐波堆（像元音），全程叠一层**确定性宽带噪声**
/// （LCG 白噪声，模拟风扇 / HVAC / 电流这类稳态底噪——正是 RNNoise 的强项）。
/// 噪声是稳态的（跨段不重置），所以既能量"静音段被压下去多少"，也能量"说话时
/// 高频底噪被压下去多少"。首段刻意就是说话，用来观察 RNNoise 的首帧淡入是否会造成
/// 可感知的瞬态（实测只有 1 帧、可忽略）。
fn material() -> (Vec<i16>, Vec<(usize, usize, bool)>) {
    let mut state: u32 = 0x1234_5678;
    let mut noise = |amp: f32| -> f32 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let u = (state >> 8) as f32 / ((1u32 << 24) as f32) - 0.5; // [-0.5, 0.5)
        amp * u
    };
    let voice = |t: f32| {
        let p = 2.0 * std::f32::consts::PI * t / RATE as f32;
        let env = 0.35 + 0.15 * (2.0 * std::f32::consts::PI * 3.0 * t / RATE as f32).sin();
        0.25 * env
            * ((VOICE_HZ[0] * p).sin()
                + 0.6 * (VOICE_HZ[1] * p).sin()
                + 0.3 * (VOICE_HZ[2] * p).sin())
    };
    let noise_amp = 0.16;

    let mut samples = Vec::new();
    let mut segs = Vec::new();
    // ★ 第一段刻意就是**说话**：RNNoise 首帧有淡入，但只有 1 帧、不会把整段人声当
    //   噪声记下来（那是谱减法的旧坑，RNNoise 没有）。
    let plan = [
        (0.6, true),
        (0.6, false),
        (0.6, true),
        (0.6, false),
        (0.6, true),
    ];
    for (secs, talking) in plan {
        let n = (secs * RATE as f32) as usize;
        let start = samples.len();
        for i in 0..n {
            let t = (start + i) as f32;
            let v = if talking { voice(t) } else { 0.0 } + noise(noise_amp);
            samples.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        segs.push((start, samples.len(), talking));
    }
    (samples, segs)
}

fn rms_dbfs(s: &[i16]) -> f32 {
    if s.is_empty() {
        return -120.0;
    }
    let sum: f64 = s.iter().map(|&v| (v as f64) * (v as f64)).sum();
    to_db(((sum / s.len() as f64).sqrt() / 32768.0) as f32)
}

fn to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// 单频点幅度（Goertzel），满幅正弦归一化到 1.0。
fn goertzel(samples: &[i16], freq: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * freq / RATE as f32;
    let cw = w.cos();
    let coeff = 2.0 * cw;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in samples {
        let s0 = v as f32 / 32768.0 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let re = s1 - s2 * cw;
    let im = s2 * w.sin();
    2.0 * (re * re + im * im).sqrt() / samples.len().max(1) as f32
}

/// 一组频率上的平均电平（dBFS）。
fn band_db(samples: &[i16], freqs: &[f32]) -> f32 {
    let sum: f32 = freqs.iter().map(|&f| goertzel(samples, f).powi(2)).sum();
    to_db((sum / freqs.len() as f32).sqrt())
}

/// 段的"掐头去尾"（避开段边界的过渡）。
fn trimmed(a: usize, b: usize) -> (usize, usize) {
    let pad = (b - a) / 10;
    (a + pad, b - pad)
}

struct Report {
    /// 静音段整体底噪（dBFS）。
    quiet: f32,
    /// 各说话段里**高频底噪**的电平（dBFS）—— 人耳听到的嘶嘶声。
    hf: Vec<f32>,
    /// 各说话段里**人声谐波**相对输入的电平变化（dB，负数 = 被削）。
    voice_rel: Vec<f32>,
}

fn measure(setting: &str, input: &[i16], segs: &[(usize, usize, bool)]) -> Report {
    let out = denoise_offline(input, 1, setting);
    // RNNoise 无 STFT 延迟；仅在最前面跳过 1 帧（480 样本）避开首帧淡入瞬态。
    let skip = 480.min(out.len());
    let mut quiet = Vec::new();
    let mut hf = Vec::new();
    let mut voice_rel = Vec::new();
    for &(a, b, talking) in segs {
        let (lo, hi) = trimmed(a, b);
        if lo >= hi {
            continue;
        }
        if !talking {
            let (lo, hi) = (lo.saturating_sub(skip), hi.saturating_sub(skip));
            if lo < hi {
                quiet.extend_from_slice(&out[lo..hi]);
            }
            continue;
        }
        let (olo, ohi) = (lo.saturating_sub(skip), hi.saturating_sub(skip));
        if olo >= ohi {
            continue;
        }
        hf.push(band_db(&out[olo..ohi], &[HF_HZ]));
        // 人声按"输出/输入"的比值看，抵消各段本身的包络差异。
        voice_rel.push(band_db(&out[olo..ohi], &VOICE_HZ) - band_db(&input[lo..hi], &VOICE_HZ));
    }
    Report {
        quiet: rms_dbfs(&quiet),
        hf,
        voice_rel,
    }
}

impl Report {
    /// 说话段高频底噪的**稳态**值（排除首段 —— 首帧淡入的一次性瞬态，见上文）。
    fn hf_steady(&self) -> f32 {
        let r = if self.hf.len() > 1 {
            &self.hf[1..]
        } else {
            &self.hf[..]
        };
        r.iter().sum::<f32>() / r.len().max(1) as f32
    }
    /// 人声谐波的稳态变化（dB）。
    fn voice_steady(&self) -> f32 {
        let r = if self.voice_rel.len() > 1 {
            &self.voice_rel[1..]
        } else {
            &self.voice_rel[..]
        };
        r.iter().sum::<f32>() / r.len().max(1) as f32
    }
}

#[test]
#[ignore = "需要造信号跑算法，不进快速回归；用 --ignored --nocapture 跑"]
fn denoise_modes_are_distinguishable_and_really_work() {
    let (input, segs) = material();
    let base_q = {
        let mut v = Vec::new();
        for &(a, b, talking) in &segs {
            if !talking {
                let (lo, hi) = trimmed(a, b);
                v.extend_from_slice(&input[lo..hi]);
            }
        }
        rms_dbfs(&v)
    };
    let base_hf: Vec<f32> = segs
        .iter()
        .filter(|(_, _, talking)| *talking)
        .map(|&(a, b, _)| {
            let (lo, hi) = trimmed(a, b);
            band_db(&input[lo..hi], &[HF_HZ])
        })
        .collect();
    let base_hf_steady = base_hf[1..].iter().sum::<f32>() / (base_hf.len() - 1) as f32;

    println!("\n== 降噪离屏验收（单声道 48kHz，宽带噪声素材）==");
    println!("素材：说话 / 静音 / 说话 / 静音 / 说话，每段 0.6s —— 第一段就是说话");
    println!("输入：静音段底噪 {base_q:.1} dBFS；说话段里的高频底噪 {base_hf_steady:.1} dBFS");
    println!(
        "\n{:<10} {:>13} {:>19} {:>19}",
        "档位", "静音段底噪", "说话段高频底噪", "人声谐波变化"
    );

    let rows: Vec<(&str, Report)> = [
        ("关闭", "off"),
        ("弱", "weak"),
        ("中", "medium"),
        ("强", "strong"),
    ]
    .into_iter()
    .map(|(name, setting)| {
        let r = measure(setting, &input, &segs);
        println!(
            "{:<10} {:>10.1} dB {:>16.1} dB {:>16.1} dB",
            name,
            r.quiet,
            r.hf_steady(),
            r.voice_steady(),
        );
        (name, r)
    })
    .collect();

    let (_, off) = &rows[0];
    let (sp_w_n, sp_w) = &rows[1];
    let (sp_m_n, sp_m) = &rows[2];
    let (sp_s_n, sp_s) = &rows[3];

    println!("\n-- 判定 --");
    // ① RNNoise 必须真的把静音段的稳态噪声压下去（"降噪生效"的最小判据）。
    assert!(
        sp_s.quiet < off.quiet - 3.0,
        "RNNoise·强 该把静音段底噪压下 3 dB 以上：{:.1} → {:.1}",
        off.quiet,
        sp_s.quiet
    );
    // ② 说话时也在降：说话段里的高频底噪比输入低（RNNoise 相对门控的核心价值）。
    //    Goertzel 单频点在白噪声上读数有抖动，所以只锁"方向正确"（> 0 dB 降幅），
    //    具体降了多少看上面打印的数字。
    assert!(
        sp_s.hf_steady() < base_hf_steady - 1.0,
        "{} 在说话段应把高频底噪压低：输入 {:.1} → {:.1} dB",
        sp_s_n,
        base_hf_steady,
        sp_s.hf_steady()
    );
    // ③ 只能压噪声、不能削人声：说话段的谐波电平要基本保住。
    for (n, r) in [(*sp_w_n, sp_w), (*sp_m_n, sp_m), (*sp_s_n, sp_s)] {
        assert!(
            r.voice_steady() > -4.0,
            "{n} 把说话段的人声谐波削掉了 {:.1} dB，过头了",
            r.voice_steady()
        );
    }
    // ④ 三档可分辨、且都比关更干净（干湿混合比：弱 50% / 中 80% / 强 100%）。
    //    ★ 不锁"单调"：RNNoise 输出与原始噪声去相关，80% 湿偶尔会比 100% 湿残留略低，
    //      这是湿干混合的物理结果，不是 bug（详见 audio_capture 单测
    //      denoise_strength_changes_the_output）。这里只锁"两两不同 + 都低于关"。
    let off_q = off.quiet;
    for (n, r) in [(*sp_w_n, sp_w), (*sp_m_n, sp_m), (*sp_s_n, sp_s)] {
        assert!(
            r.quiet < off_q - 1.0,
            "{n} 没比关闭更干净：{off_q:.1} vs {:.1}",
            r.quiet
        );
    }
    assert!(
        (sp_w.quiet - sp_m.quiet).abs() > 0.5
            && (sp_m.quiet - sp_s.quiet).abs() > 0.5
            && (sp_w.quiet - sp_s.quiet).abs() > 0.5,
        "三档应两两可分辨：弱 {:.1} / 中 {:.1} / 强 {:.1}",
        sp_w.quiet,
        sp_m.quiet,
        sp_s.quiet
    );
    println!("全部判定通过");
}
