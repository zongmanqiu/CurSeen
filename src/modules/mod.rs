//! 模块 trait：所有特效模块（拖尾、击键回显、屏幕书写）的统一接口。

pub mod clipboard;
pub mod drawing;
pub mod key_display;
pub mod key_sound;
pub mod magnifier;
pub mod sticky_note;
pub mod trail;
pub mod virtual_keyboard;
use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::InputEvent;
use crate::core::tick::Tick;

/// 模块 trait。
pub trait Module {
    /// 模块是否启用。
    fn enabled(&self, config: &Config) -> bool;

    /// 每帧调用：更新内部状态、往 DrawList 里塞指令。
    fn tick(&mut self, tick: &Tick, config: &Config, input: &Option<InputEvent>);

    /// 绘制：把当前帧的指令写入 DrawList。
    fn draw(&self, draw: &mut DrawList, config: &Config);

    /// ★ 内存回收：本模块**被关掉的那一刻**释放它独占的全部资源。
    ///
    /// 为什么需要它：模块多半持有随使用增长的缓冲——书写的撤销栈（每笔一串点）、
    /// 拖尾的用户图片/GIF 帧、冻结帧（`Rc<RgbaBitmap>`，1080p 约 8 MB、4K 约 33 MB）——
    /// 而这些东西在 "只是把开关拨到关" 之后仍然留着，**用户看到功能关了，内存却没掉**。
    /// 常驻工具最不该有的就是这个：它不是泄漏（还能拿回来），但用户无从判断。
    ///
    /// 默认实现为空 No-op：只有确实持有大缓冲的模块才需要重写（见 `drawing` / `trail`）。
    /// `Runtime` 在每个模块 **enabled 由 true 变 false** 的那帧调用它，调用点只有一处。
    fn release_memory(&mut self) {}
}
