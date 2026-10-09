//! 提示浮字（"屏幕标注模式"/"已退出绘图"）的生命周期。

use crate::core::draw::DrawList;
use crate::core::geometry::Rect;
use crate::platform::display::ScreenRect;
use std::cell::Cell;

/// 提示浮字。生命周期 = 2.0 秒，线性淡出。
///
/// ★ 另带一个**全屏白闪**通道（`flash`）：截图成功时的不可错过的反馈——
/// 整屏白幕 0.35s 内从 40% 透明度淡到 0。旧反馈只有屏幕中央一行 28pt 白字、
/// 且 park 态解锁有 ~250ms 延迟，用户实测“没感觉”。
pub struct Notify {
    text: String,
    remain: f32,
    last_bbox: Cell<Option<Rect>>,
    /// 全屏白闪剩余时间（秒）。截图成功时置位，见 [`Self::flash`]。
    flash: f32,
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

impl Notify {
    pub const DURATION: f32 = 2.0;
    pub const FONT_PT: f32 = 28.0;
    /// 全屏白闪时长（秒）。
    pub const FLASH: f32 = 0.35;

    pub fn new() -> Self {
        Self {
            text: String::new(),
            remain: 0.0,
            last_bbox: Cell::new(None),
            flash: 0.0,
        }
    }

    /// 显示提示文字。
    pub fn show(&mut self, text: &str) {
        self.text = text.to_string();
        self.remain = Self::DURATION;
    }

    /// 触发一次全屏白闪（截图成功的强反馈，0.35s 淡出）。
    pub fn flash(&mut self) {
        self.flash = Self::FLASH;
    }

    /// 白闪是否还在播放（帧门据此继续排帧）。
    pub fn flashing(&self) -> bool {
        self.flash > 0.0
    }

    /// 绘制全屏白闪（在所有内容之上）。返回是否绘制了。
    pub fn draw_flash(&self, list: &mut DrawList, screen: ScreenRect) -> bool {
        if self.flash <= 0.0 {
            return false;
        }
        let alpha = (self.flash / Self::FLASH * 0.40 * 255.0) as u8;
        list.fill_rect(
            Rect {
                x: screen.x as f32,
                y: screen.y as f32,
                w: screen.w as f32,
                h: screen.h as f32,
            },
            crate::core::color::Argb::new(alpha, 0xFF, 0xFF, 0xFF),
        );
        true
    }

    /// 每帧更新。返回 false 表示已消失。
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.flash > 0.0 {
            self.flash = (self.flash - dt).max(0.0);
        }
        if self.remain <= 0.0 {
            return self.flash > 0.0;
        }
        self.remain -= dt;
        self.remain > 0.0 || self.flash > 0.0
    }

    /// 绘制。返回需要标脏的区域（当前帧和上一帧的并集）。
    ///
    /// 传**虚拟桌面整块**而不是尺寸：世界坐标的原点是虚拟桌面原点（副屏在左时 x 为负），
    /// 只按尺寸居中会让提示浮字跑到主屏上去。
    pub fn draw(&self, list: &mut DrawList, screen: ScreenRect) -> Option<Rect> {
        let prev = self.last_bbox.get();

        if self.text.is_empty() || self.remain <= 0.0 {
            // 消失的那一帧要用上一帧的 bbox 标脏
            self.last_bbox.set(None);
            return prev;
        }

        let alpha = self.remain / Self::DURATION;
        let alpha_u8 = (alpha * 255.0) as u8;
        let fg = crate::core::color::Argb::new(alpha_u8, 0xFF, 0xFF, 0xFF);
        // ★ 深色底框 + 白字改走**共享横幅函数**（2026-09-22）：底框与文字由同一
        //   center 推导、垂直居中由 text_centered 保证——旧实现手算 baseline
        //   （+text_h/3 魔法数），底框和文字没对齐（用户实测"做得太差"）。
        //   GIF/MP4 倒计时、计时器同函数，观感与对齐全库一致。
        let bg = crate::core::color::Argb::new((alpha_u8 as f32 * 0.72) as u8, 0x20, 0x22, 0x26);
        let box_ =
            list.draw_centered_banner(screen, &self.text, Self::FONT_PT, 18.0, 10.0, bg, fg, 8.0);

        let cur_bbox = box_;
        let dirty = match prev {
            Some(p) => p.union(cur_bbox),
            None => cur_bbox,
        };
        self.last_bbox.set(Some(cur_bbox));

        Some(dirty)
    }

    /// 是否正在显示（文字或白闪任一活跃）。
    pub fn is_showing(&self) -> bool {
        self.remain > 0.0 || self.flash > 0.0
    }

    /// 剩余毫秒。`None` = 已经消失（帧门据此停止排帧）。
    pub fn remain_ms(&self) -> Option<u32> {
        let r = self.remain.max(self.flash);
        if r <= 0.0 {
            None
        } else {
            Some((r * 1000.0) as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_show_and_tick() {
        let mut n = Notify::new();
        assert!(!n.is_showing());
        n.show("测试");
        assert!(n.is_showing());
        assert!(n.tick(1.0));
        assert!(!n.tick(1.5));
        assert!(!n.is_showing());
    }

    #[test]
    fn notify_draw_empty() {
        let n = Notify::new();
        let mut list = DrawList::new();
        let dirty = n.draw(
            &mut list,
            ScreenRect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
        );
        assert!(dirty.is_none());
    }

    #[test]
    fn notify_centers_on_the_virtual_desktop_origin() {
        // 副屏在左：虚拟桌面原点 x = -1920。提示浮字必须落在虚拟桌面中间，
        // 而不是主屏（0..1920）中间——按尺寸居中会让它跑到主屏上去。
        let mut n = Notify::new();
        n.show("屏幕标注模式");
        let mut list = DrawList::new();
        let dirty = n
            .draw(
                &mut list,
                ScreenRect {
                    x: -1920,
                    y: 0,
                    w: 3840,
                    h: 1080,
                },
            )
            .expect("显示期间应有脏区");
        let center = dirty.x + dirty.w / 2.0;
        assert!(
            (center - 0.0).abs() < 40.0,
            "应居中于虚拟桌面（x≈0），实际中心 {center}"
        );
    }
}
