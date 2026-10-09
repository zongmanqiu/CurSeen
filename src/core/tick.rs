//! 帧时钟：负责 dt 计算与稳定化。
//!
//! ★ 纯逻辑：时间来源用 `std::time::Instant`，不依赖 `platform`（core 不得引用 platform）。
//! 只需相邻帧的相对间隔，不需要开机绝对时刻，故 `Instant` 足够且天然单调。

use std::time::Instant;

/// 帧时钟。
pub struct FrameClock {
    last: Instant,
    smoothed_ms: f32,
    elapsed_ms: f64,
}

/// 一帧的时间信息。
pub struct Tick {
    pub raw_ms: f32,
    pub smooth_ms: f32,
    pub dt_ms: f32,
    pub elapsed_ms: f64,
}

impl FrameClock {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            smoothed_ms: 16.0,
            elapsed_ms: 0.0,
        }
    }

    /// 推进一帧，返回 Tick。
    /// capped_dt_ms = clamp(raw, 0.5, 50.0)——夹住长暂停造成的巨大 dt。
    pub fn tick(&mut self) -> Tick {
        let now = Instant::now();
        // `saturating_duration_since` 吃掉时钟回拨（极罕见），避免负数 dt。
        let raw = now.saturating_duration_since(self.last).as_secs_f32() * 1000.0;
        self.last = now;

        // 指数滑动平均抑制抖动
        self.smoothed_ms = self.smoothed_ms * 0.9 + raw * 0.1;
        let capped = self.smoothed_ms.clamp(0.5, 50.0);
        self.elapsed_ms += capped as f64;

        Tick {
            raw_ms: raw,
            smooth_ms: self.smoothed_ms,
            dt_ms: capped,
            elapsed_ms: self.elapsed_ms,
        }
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}
