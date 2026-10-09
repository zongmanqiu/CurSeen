//! 数学工具：clamp、lerp、缓动、确定性 RNG。

pub fn clamp_f(v: f32, lo: f32, hi: f32) -> f32 {
    v.clamp(lo, hi)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn ease_out_quad(t: f32) -> f32 {
    t * (2.0 - t)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t1 = t - 1.0;
    t1 * t1 * t1 + 1.0
}

pub fn ease_in_out_sine(t: f32) -> f32 {
    -((std::f32::consts::PI * t).cos() - 1.0) / 2.0
}

/// 确定性 RNG（xorshift64*）。粒子随机用它，保证同种子同结果。
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // xorshift64* 不接受 0 种子
        Self(if seed == 0 { 1 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }

    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_f32() * (hi - lo) as f32) as i32
    }

    /// 单位圆上的随机方向。
    pub fn unit_vec(&mut self) -> (f32, f32) {
        let a = self.next_f32() * std::f32::consts::TAU;
        (a.cos(), a.sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn rng_range() {
        let mut rng = Rng::new(1);
        for _ in 0..1000 {
            let v = rng.range_f32(2.0, 5.0);
            assert!((2.0..5.0).contains(&v));
        }
    }
}
