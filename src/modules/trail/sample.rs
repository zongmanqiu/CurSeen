//! 采样环形缓冲。捕获线程投递、UI 线程消费。容量固定 1024，全程零分配。

/// 鼠标采样点。
#[derive(Clone, Copy, Debug)]
pub struct MouseSample {
    pub x: f32,
    pub y: f32,
    pub t_ms: u32,
}

/// 定长环形缓冲。满时覆盖最旧的点（而非丢弃最新的），
/// 这样画面仍连续，等价于"降低采样率"。
pub struct SampleQueue {
    buf: [MouseSample; Self::CAP],
    head: usize,
    len: usize,
    overflow_count: u64,
}

/// drain 迭代器：不拷贝，原地标记逻辑清空。
pub struct SampleDrain<'a> {
    buf: &'a [MouseSample; 1024],
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for SampleDrain<'a> {
    type Item = MouseSample;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let s = self.buf[self.pos];
        self.pos = (self.pos + 1) % 1024;
        self.remaining -= 1;
        Some(s)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for SampleDrain<'_> {}

impl SampleQueue {
    pub const CAP: usize = 1024;

    pub const fn new() -> Self {
        // 安全的 const 初始化：用 MaybeUninit 等效的全零
        Self {
            buf: [MouseSample {
                x: 0.0,
                y: 0.0,
                t_ms: 0,
            }; Self::CAP],
            head: 0,
            len: 0,
            overflow_count: 0,
        }
    }

    pub fn push(&mut self, s: MouseSample) {
        if self.len == Self::CAP {
            // 覆盖最旧的点
            self.overflow_count += 1;
        } else {
            self.len += 1;
        }
        self.buf[self.head] = s;
        self.head = (self.head + 1) % Self::CAP;
    }

    /// 一次取走所有待处理点。返回迭代器，不拷贝数据。
    pub fn drain(&mut self) -> SampleDrain<'_> {
        let start = if self.len < Self::CAP {
            (self.head + Self::CAP - self.len) % Self::CAP
        } else {
            self.head
        };
        let count = self.len;
        self.len = 0;
        self.head = 0;
        SampleDrain {
            buf: &self.buf,
            pos: start,
            remaining: count,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.head = 0;
    }

    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }
}

impl Default for SampleQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_drain() {
        let mut q = SampleQueue::new();
        q.push(MouseSample {
            x: 1.0,
            y: 2.0,
            t_ms: 100,
        });
        q.push(MouseSample {
            x: 3.0,
            y: 4.0,
            t_ms: 200,
        });
        assert_eq!(q.len(), 2);
        let v: Vec<_> = q.drain().collect();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].x, 1.0);
        assert_eq!(v[1].x, 3.0);
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn overflow_overwrites_oldest() {
        let mut q = SampleQueue::new();
        for i in 0..1030 {
            q.push(MouseSample {
                x: i as f32,
                y: 0.0,
                t_ms: i,
            });
        }
        assert_eq!(q.len(), 1024);
        assert!(q.overflow_count() > 0);
        let v: Vec<_> = q.drain().collect();
        // The oldest points (0..6) were overwritten
        assert_eq!(v[0].x, 6.0);
    }

    #[test]
    fn drain_exact_size() {
        let mut q = SampleQueue::new();
        for i in 0..10 {
            q.push(MouseSample {
                x: i as f32,
                y: 0.0,
                t_ms: i,
            });
        }
        let d = q.drain();
        assert_eq!(d.len(), 10);
    }
}
