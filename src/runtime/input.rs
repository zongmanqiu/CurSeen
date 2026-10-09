//! 输入分发与快捷键判定。
//!
//! `feed` 是钩子事件进 Runtime 的唯一入口；`handle_action_key` 是功能快捷键的总表。
//! 下面几个**纯函数**（`shot_edit_entry` / `resolve_snap_hotkey`）刻意与硬件读取分开：
//! `ctrl_held()` 读的是钩子维护的全局静态，单测造不出「Ctrl 按着」——把判定拆成纯函数
//! 才能钉住真值表。

use super::*;
use crate::core::hotkeys::global_safe;

/// 快捷键串**或它的 Ctrl 变体**命中即算命中（"按住 Ctrl 让这把键多出一种语义"）。
///
/// 存在的理由：判断"要不要为这条键唤醒一帧"时，两种含义是**同一个入口**，分不出也无所谓；
/// 而真正分派模式的地方（[`resolve_snap_hotkey`]）必须分开判 —— 那里要用
/// `with_ctrl(spec)` 的结果去决定"这次是普通截图还是截图编辑态"。
///
/// `spec` 为空、或它本身已经含 Ctrl 时，`with_ctrl` 返回空串，`matches` 对空规格恒
/// `false`，所以这里天然退化成"只比原规格"。
pub(crate) fn hk_matches_or_ctrl(spec: &str, key: KeyCode, mods: Modifiers) -> bool {
    hk_matches(spec, key, mods) || hk_matches(&with_ctrl(spec), key, mods)
}

/// 截图 / 贴图三把快捷键解析出的"该走哪条路"。纯数据，便于单测把真相表钉死。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapHotkey {
    /// 截图（只进剪贴板 + 留档）。
    Shot,
    /// 截图并贴图（进剪贴板 + 钉到桌面）。
    ShotPin,
    /// 贴图（把剪贴板里的内容钉到桌面）。
    PinClipboard,
    /// ★ 长截图（滚动截图）快捷键：**开始 / 结束一键开关**（同录屏）。空闲按下 = 进框选；
    /// 采集进行中按下 = 结束并交付。没有 Ctrl 变体（长截图没有编辑态语义）。
    Scroll,
    /// ★ 宽截图快捷键（2026-10-01）：与 [`SnapHotkey::Scroll`] 同一套会话状态机，
    /// 差别只在空闲分支进的框选模式（`ScrollWide`，方向向右）。
    ScrollWide,
    /// 截图**编辑态**（框完不停，可调选区、可标注）；`pin` = 确认后是否还要钉到桌面。
    ShotEdit { pin: bool },
}

/// 三把截图 / 贴图快捷键 → [`SnapHotkey`]（`None` = 这一下不归它们管）。
///
/// # 「按住 Ctrl 进编辑态」是怎么落地的
///
/// 只有前两把有编辑态，判定是**一对**：本体 + 它的 **Ctrl 变体**（`Alt+S` → `Ctrl+Alt+S`，
/// 由 [`with_ctrl`] 拼出来）。所以"截图并贴图"走的是**同一条** `ShotEdit`，
/// 两者只在 `pin` 上分岔 —— 它从来不是另写的一个函数。这个函数是**唯一**的判定点：
/// 从前它散在 `handle_action_key` 的一串 `if` 里，顺序本身承载语义却没人写下来。
///
/// ★★ **变体必须排在本体之后（本体优先）。** 撞弦时让本体赢，理由是"用户按下的就是配置里
///   写的那把键"，这是唯一不让人意外的解释；反过来（变体优先）会把一把键的语义静默换掉。
///   真会撞的例子：`hotkey = Ctrl+F3` 与 `hotkey_pin = F3` —— 前者既是截图本体、又是后者的
///   Ctrl 变体。变体优先时按 `Ctrl+F3` 变成"截图并贴图 + 编辑"，两把键的 Ctrl 路**同时**错位。
///   这类配置由 [`crate::core::config::Config::migrate_snap_hotkey_shadowing`] 在加载时拆开。
///
/// ★★ **规格本身已含 Ctrl 时，这把键没有 Ctrl 变体。** `with_ctrl` 对含 Ctrl 的规格返回
///   空串（"再按住 Ctrl"物理上不存在另一种按法），于是该键只剩本体一条路 —— 进不了编辑态。
///   出厂值必须避开这种键。旧默认 `hotkey_pin = Ctrl+Alt+Q` 曾栽在这里：用户报"截图按住
///   Ctrl 能编辑，截图并贴图按住 Ctrl 毫无反应"（2026-09-23），改 `Alt+Q` 后两把键各自的
///   两条路都在；★ 2026-09-26 起出厂干脆**全留空**（用户口径），这条坑只剩用户自己配
///   含 Ctrl 的键才会踩 —— 那时
///   [`crate::core::config::Config::snap_keys_missing_ctrl_variant`] 会在加载时记一条日志，
///   不让它继续静默。
/// 这次框选确认该不该转进**截图编辑态**（`Some(pin)` = 该进，`pin` = 确认后是否还贴图）。
///
/// 两个入口是**或**关系（见 [`Runtime::confirm_snap`] 的长注释）：
///   · `SnapMode::ShotEdit` —— 按快捷键那一刻就按着 Ctrl，模式在 [`resolve_snap_hotkey`]
///     里就定好了，此时**直接采用它的 `pin`、不重判 Ctrl**（否则"Ctrl+截图并贴图"
///     在用户松开 Ctrl 后会退化成不贴图）；
///   · `Shot` / `ShotPin` + 此刻按着 Ctrl —— "先触发、再按住 Ctrl 拖框"。
///
/// 抽成纯函数是为了**可测**：`ctrl_held()` 读的是键盘钩子维护的全局静态，单测造不出
/// "Ctrl 按着"这个状态；把它与"读硬件"拆开后，真值表就能直接钉死在测试里。
///
/// GIF 选框**必须**返回 `None`：那不是在截图，是要"录哪一块"，它不该有编辑态 ——
/// 否则按住 Ctrl 框 GIF 区域会掉进一个只剩取消的界面。
pub(crate) fn shot_edit_entry(mode: snap::SnapMode, ctrl: bool) -> Option<bool> {
    match mode {
        snap::SnapMode::ShotEdit { pin } => Some(pin),
        snap::SnapMode::Shot if ctrl => Some(false),
        snap::SnapMode::ShotPin if ctrl => Some(true),
        _ => None,
    }
}

pub(crate) fn resolve_snap_hotkey(
    s: &crate::core::config::ScreenshotSettings,
    key: KeyCode,
    mods: Modifiers,
) -> Option<SnapHotkey> {
    // ① 本体：精确比对。`hk_matches` 对空串恒假，所以不必再逐条判空。
    if hk_matches(&s.hotkey, key, mods) {
        return Some(SnapHotkey::Shot);
    }
    if hk_matches(&s.hotkey_pin, key, mods) {
        return Some(SnapHotkey::ShotPin);
    }
    if hk_matches(&s.hotkey_pin_clipboard, key, mods) {
        return Some(SnapHotkey::PinClipboard);
    }
    // ★ 长截图键：一键开关，本体唯一（排在 Ctrl 变体之前没有语义冲突——它没有变体）。
    if hk_matches(&s.hotkey_scroll, key, mods) {
        return Some(SnapHotkey::Scroll);
    }
    // ★ 宽截图键（2026-10-01）：与长截图同一套交互，方向向右。也必须排在 Ctrl 变体之前。
    if hk_matches(&s.hotkey_scroll_wide, key, mods) {
        return Some(SnapHotkey::ScrollWide);
    }
    // ② Ctrl 变体：按住 Ctrl 再按同一把键 = 进编辑态（唯一的"多义"入口）。
    if hk_matches(&with_ctrl(&s.hotkey), key, mods) {
        return Some(SnapHotkey::ShotEdit { pin: false });
    }
    if hk_matches(&with_ctrl(&s.hotkey_pin), key, mods) {
        return Some(SnapHotkey::ShotEdit { pin: true });
    }
    None
}

impl Runtime {
    /// 接收输入事件。
    pub fn feed(&mut self, event: InputEvent) {
        // ★ 截图编辑态：落在选区交互带上的按下，以及它之后的拖动/抬手，先被本会话吃掉 ——
        //   不吃的话同一条 `MouseDown` 还会走到标注那边，用户想调框却落成一笔墨。
        if self.edit_drag(&event) {
            return;
        }
        // 拖尾走独立采样队列；其余模块在 `frame()` 里按队列逐条处理。
        match event {
            InputEvent::MouseMove { x, y, time_ms } => {
                self.trail.push_sample(x as f32, y as f32, time_ms as u32);
            }
            InputEvent::MouseDown { button, x, y, .. } => {
                // ★ 虚拟键盘优先：点在键帽上就被它吃掉 —— 注入按键，且**不**给拖尾留下
                //   一笔墨迹（吞掉即不再入队，后面的模块看不到这条事件）。吞不吞的判据
                //   与鼠标钩子同源（都查 `platform/input/hit`），见 `vk_pointer_begin`。
                if self.vk_pointer_begin(button, x, y) {
                    return;
                }
                self.trail.on_click(button, x as f32, y as f32);
                // ★ 点贴图以外的地方 → 取消贴图选中态。做成几何判定（不用窗口消息）：
                //   贴图窗是独立窗口，它自己的 `WM_LBUTTONDOWN` 与这里收到的全局钩子事件
                //   谁先谁后不确定；判据只看"点有没有落在选中的那张贴图矩形里"，与顺序无关。
                self.note_pointer_down(x, y);
            }
            InputEvent::MouseUp { button, .. } => {
                if self.vk_pointer_end(button) {
                    return;
                }
            }
            InputEvent::MouseWheel {
                delta, horizontal, ..
            }
                // ★ 缩放滚轮（**内置快捷键 Ctrl+Win+滚轮**，钩子只在组合命中时才
                //   转发到这里——组合、为什么选它、冲突实测，见
                //   `platform::input::mouse` 的 WM_MOUSEWHEEL 注释）。
                //   方向与 Windows 放大镜一致：前推（delta>0）放大、后拉缩小。
                //   横向倾斜滚轮只服务击键回显（回显武装时钩子才镜像），与倍率无关。
                if !horizontal => {
                    let dir = if delta > 0 { 1 } else { -1 };
                    if self.zoom_step(dir) {
                        return;
                    }
                }
            _ => {}
        }
        self.input_queue.push(event);
    }

    /// 鼠标落在虚拟键盘的键帽上 → 注入一次"按下"。返回是否吃掉这条事件。
    ///
    /// 命中判定读的是 [`crate::platform::input::hit`] 那块共享区，**不在这里重算几何**：
    /// 鼠标钩子已经用同一份数据决定过"吞不吞"，两边同源才不会出现"钩子吞了、这里没认出"
    /// （点了没反应）或者反过来（下层也收到一下）。
    pub(crate) fn vk_pointer_begin(&mut self, button: MouseButton, x: i32, y: i32) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let s = &self.config.virtual_keyboard;
        if !s.enabled || s.click_through {
            return false;
        }
        // ★ 判据与鼠标钩子**同源**（都调 `hit::claim`）：钩子吞了而这里不注入 = 这次点击
        //   凭空消失；这里注入了而钩子没吞 = 下层应用也收到一下（双触发）。
        //   `claim` 会一并排除"落点压在我们自己窗口上"的点（面板/取色器弹窗）——
        //   那些点击归它们，虚拟键盘不许抢。
        let Some(code) = crate::platform::input::hit::claim(x, y) else {
            log::debug!("vk_pointer_begin：点 x={x} y={y} 未被虚拟键盘接管");
            return false;
        };
        // ★★ 鼠标键帽走**另一条路**（2026-09-26 修）：鼠标键不是键盘虚拟键，
        //   `vk::to_vk_ext` 对 `MouseLeft/Right/...` 根本没有映射，`send_key` 必然返回
        //   false 并记一条 warn。旧实现照样调它、然后**无条件吃掉这次点击** —— 于是
        //   点鼠标键帽 = 注入失败（日志里一条 warn）+ 事件被吞（键帽自己也不亮）+ 点击
        //   凭空消失（穿透没开时下层应用也收不到）。三头落空，就是用户报的"没反应"。
        //
        //   鼠标键帽本来就只有**高亮**语义（方案里写它是为了显示"我按了左键"）：
        //   这里直接点亮它、吃掉点击，不做任何注入。
        if is_mouse_key(code) {
            log::debug!("vk_pointer_begin：命中鼠标键帽 {code:?}（只高亮，不注入）");
            self.vk.note_press(code, true);
            self.vk_pointer = Some(code);
            return true;
        }
        // 注入成功才记下这颗键：失败（比如前台窗口权限更高）时不该留一个永远等不到
        // 抬起的按下态。
        let injected = crate::platform::input::inject::send_key(code, true);
        log::info!("vk_pointer_begin：命中 {code:?} x={x} y={y} → send_key={injected}");
        if injected {
            self.vk_pointer = Some(code);
        }
        true
    }

    /// 松开鼠标 → 注入"抬起"。返回是否吃掉这条事件。
    pub(crate) fn vk_pointer_end(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let Some(code) = self.vk_pointer.take() else {
            return false;
        };
        if is_mouse_key(code) {
            // 鼠标键帽那一路没有注入过"按下"（也没有可注入的 VK），抬起自然也不注入
            // —— 只把高亮交给保持 / 淡出时间线。
            self.vk.note_press(code, false);
        } else {
            crate::platform::input::inject::send_key(code, false);
        }
        true
    }

    /// 标注输入是否该挂起。
    ///
    /// 「打开对话框」与「标注」是互斥的两种上下文：对话框要的是鼠标能点到控件、键盘能敲字，
    /// 而标注要的是鼠标去画笔、键盘去切工具。两者同时生效时，用户点一下按钮就会多出一笔。
    /// 所以只要有一个我们的 UI 开着，就把标注输入整体挂起（键鼠一起，见
    /// `DrawingModule::set_suspension`）。
    ///
    /// ★ 裁剪窗也在名单里（2026-09-20 夜补）：它不再是置顶窗，覆盖层会给它让路
    ///   （见 [`Self::gif_trim_open`]）—— **让路与挂起是配套的一对**。只让路不挂起，
    ///   用户在裁剪窗上点一下就在画布上多出一笔（面板当初的坑，见
    ///   `layer::mod` 里 `click_through` 的说明）。
    pub fn input_suspended(&self) -> bool {
        self.panel.is_some() || self.text_window_open || self.gif_trim_open
    }

    /// 是否正在捕获快捷键（面板开着且某个快捷键框处于捕获态）。
    ///
    /// `UiMsg::Input` 据此决定是否踢一帧：捕获态下按键要进 `frame()` 被
    /// [`Self::try_capture_hotkey`] 吃掉，而面板开着时覆盖层可能正 `Idle` 不出帧。
    /// ★ 2026-09-29 起脚本热键捕获框（`type = "hotkey"` 参数）也算捕获态——
    ///   固定字段与脚本参数两套捕获互斥使用，同一时刻至多一个在等键。
    pub fn is_capturing_hotkey(&self) -> bool {
        self.panel.as_ref().is_some_and(|p| {
            p.hotkey_capture.get().is_some() || p.script_hotkey_cap.get().is_some()
        })
    }

    /// 快捷键捕获：面板开着且处于捕获态时，把这条 `KeyDown` 写回对应信号并清零捕获态。
    ///
    /// 返回 `true` 表示事件已被捕获消费（调用方应跳过 `drawing.tick` / `key.tick`）。
    /// 修饰键-only 的按下（如单独按住 `Ctrl`）不消费主键、保持捕获态继续等；拿不到主键
    /// token 的未知键也吃掉，避免它透传给标注去误 toggle。
    ///
    /// ★ 两条捕获通路（2026-09-29）：脚本热键（`script_hotkey_cap`）优先查——它的
    ///   回写目标在捕获态里随身携带（sig），`set_script_hotkey_exclusive` 做同一套
    ///   互斥；固定字段走 `hotkey_capture` + `hotkey_signal` 的编译期映射，逻辑不变。
    pub(crate) fn try_capture_hotkey(&mut self, ev: &InputEvent) -> bool {
        // ── 脚本热键捕获（2026-09-29）──
        let script_target = self.panel.as_ref().and_then(|p| p.script_hotkey_cap.get());
        if let Some(target) = script_target {
            let InputEvent::KeyDown { key, mods, .. } = *ev else {
                return false;
            };
            // 修饰键本身不是主键：保持捕获态继续等真正的键（也别让它透传给标注去 toggle）。
            if is_modifier_key(key) {
                return true;
            }
            let norm = pressed(key, mods);
            if let Some(panel) = self.panel.as_ref() {
                if norm.is_empty() {
                    // 未知键等拿不到 token：吃掉，避免误触发；捕获态保持（与固定字段
                    // 同语义——按了没名堂的键不算数）。
                    return true;
                }
                // ★ 裸键校验（2026-10-01）：脚本热键是全局语义（park 态也派发），无修饰键
                //   的裸键会在打字时每一次按下都触发。拒绝、结束捕获态并留 warn——捕获框
                //   原样不动（「（空）」），用户换一把带修饰键的再按。
                if !global_safe(&norm) {
                    log::warn!(
                        "脚本热键 {} 被拒绝：无修饰键的裸键全局生效，打字时每次按下都会误触发；\
                         请配 Ctrl/Alt/Shift/Win 组合键",
                        pretty_hotkey(&norm)
                    );
                    panel.script_hotkey_cap.set(None);
                    return true;
                }
                let pretty = pretty_hotkey(&norm);
                // ★ 互斥：这把键若挂在固定字段或其它脚本参数上，一律清空（后配的赢）。
                let cleared = panel.set_script_hotkey_exclusive(&target, pretty.clone());
                for label in &cleared {
                    log::info!("脚本热键 {pretty} 设定成功，已自动清空「{label}」的旧绑定");
                }
                panel.script_hotkey_cap.set(None);
            }
            return true;
        }
        // ── 固定字段捕获（屏幕标注 / 截图 / 录制……）──
        let Some(field) = self.panel.as_ref().and_then(|p| p.hotkey_capture.get()) else {
            return false;
        };
        let InputEvent::KeyDown { key, mods, .. } = *ev else {
            return false;
        };
        // 修饰键本身不是主键：保持捕获态继续等真正的键（也别让它透传给标注去 toggle）。
        if is_modifier_key(key) {
            return true;
        }
        let norm = pressed(key, mods);
        if norm.is_empty() {
            return true; // 未知键等拿不到 token：吃掉，避免误触发
        }
        // ★ 裸键校验（2026-10-01）：除标注态工具键（`allows_bare_key`，出厂裸 1..9/0 的
        //   那批）外，所有字段都是全局语义——裸键会在打字时每一次按下都触发。拒绝时
        //   **结束捕获态**并留 warn（字段原样不动）：保持捕获态会把用户晾在"按了没反应"
        //   里，且 Esc 取消也会被这道门拦成"继续等"，越等越糊涂。
        if !field.allows_bare_key() && !global_safe(&norm) {
            let label = crate::ui::pages::global::HOTKEY_ENTRIES
                .iter()
                .find(|(_, f)| *f == field)
                .map(|(l, _)| *l)
                .unwrap_or("");
            log::warn!(
                "快捷键 {} 被拒绝（{label}）：无修饰键的裸键全局生效，打字时每次按下都会误触发；\
                 请配 Ctrl/Alt/Shift/Win 组合键",
                pretty_hotkey(&norm),
            );
            if let Some(panel) = self.panel.as_ref() {
                panel.hotkey_capture.set(None);
            }
            return true;
        }
        let pretty = pretty_hotkey(&norm);
        if let Some(panel) = self.panel.as_mut() {
            // ★ 互斥：这把键若先前挂在别的字段上，那些字段一律清空（后配的赢）。
            //   否则同一组合挂两个功能，谁生效取决于各处的匹配顺序——表现为"配了没反应"。
            let cleared = panel.set_hotkey_exclusive(field, pretty.clone());
            for (_, label) in &cleared {
                log::info!("快捷键 {pretty} 改挂到本功能，已自动清空「{label}」的旧绑定");
            }
            panel.hotkey_capture.set(None);
        }
        true
    }

    // ─── 截图 / 录屏（全局快捷键动作）──

    /// 截图 / 录屏快捷键处理（全局语义，始终生效）。
    ///
    /// 与标注 toggle 不同：标注 toggle 是「开/关标注」两个状态、必须进 `frame()` 走
    /// `DrawingModule::handle_key`；截图/录屏在这里直接执行——截图是一次动作，录屏是一键
    /// 开关。长按自动重复的 `KeyDown(repeated=true)` 不重复触发：按住截图键不会狂截图。
    pub(crate) fn handle_action_key(&mut self, ev: &InputEvent) {
        let InputEvent::KeyDown {
            key,
            mods,
            repeated,
            ..
        } = *ev
        else {
            return;
        };
        if repeated {
            return;
        }
        // ★ 长截图就绪/采集中：裸 Esc = 取消（就绪直接清掉；采集丢弃画布，不留痕迹）。
        //   排在一切快捷键之前：这两种都是"挡着"的状态，Esc 必须先用来脱身（与全屏放大
        //   的 Esc 同款优先级）。两种状态下覆盖层都不 park（`input_needs_frame` 含它们），
        //   这条必然进得了 frame()。
        if (self.scroll.is_some() || self.scroll_ready.is_some())
            && key == KeyCode::Esc
            && !(mods.ctrl || mods.alt || mods.shift || mods.win)
        {
            self.cancel_scroll();
            return;
        }
        // ★ 脚本热键（2026-09-29）：`scripts/` 下脚本在卡片参数里填了非空 `hotkey`
        //   （Str 参数，持久在 config.lua_ext.params），由这里统一派发。放在最前：
        //   它与截图/录制那类"功能状态键"没有交互——命中即脱手执行（后台线程跑脚本），
        //   本帧直接返回。
        // ★★ 2026-09-30：**必须先看这张卡片的启用开关**（memory: 用户报告"没启用 AI 写
        //   脚本，热键照样弹窗"）。参数里的热键串与启用列表是两条独立链路，旧代码只读前者，
        //   于是开关关着也照样派发。现在：
        //   - 已启用 → 派发（同一次按下只跑一个实例，见 `ext_runner` 的进程级去重）；
        //   - 未启用 → 忽略，并留一行 warn（否则用户面对"按了没反应"无从判断）。
        for (spec, id, on) in self.script_hotkeys_with_state() {
            if !hk_matches(&spec, key, mods) {
                continue;
            }
            if on {
                self.run_script_hotkey(&id);
            } else {
                log::warn!(
                    "脚本 {id} 未启用，快捷键 {spec} 已忽略（在「脚本」页勾开它的开关后才生效）"
                );
            }
            return;
        }
        // 三个截图 / 贴图快捷键 + 两把录制快捷键 + 一把裁剪快捷键，各自独立配置
        // （留空 = 停用，框内显示「（空）」）。
        // 前三个共用同一条底层链：GDI 冻结帧 → 区域选择 → 剪贴板（+ 贴图）。
        //
        // ★★ 「按住 Ctrl 进编辑态」的判定**全部**收在 [`resolve_snap_hotkey`] 里
        //   （本体优先、变体次之、含 Ctrl 的规格没有变体）—— 那三条规则写在那边的文档里，
        //   这里不再重复一遍，免得两处说法慢慢走偏。
        //
        //   判据取**按下快捷键那一刻**的修饰键，而不是框选松手那一刻的：拖框时用户多半会
        //   先把 Ctrl 松开（手要从 Ctrl 挪到鼠标），按松手时判定的话，"有没有进编辑态"
        //   就取决于手速 —— 同一个动作走出两条路，是最难复现也最难解释的一类 bug。
        //
        //   录屏 / GIF / 裁剪三把键的规格先 clone 出来：下面每次 `begin_snap` 都要
        //   `&mut self`，`self.config` 的借用活不到那之后。
        // ★ 功能开关（面板卡标题栏选框）：关 = 整组快捷键不响应。每次按键都读配置，
        //   勾选即生效，无需重启（快捷键触发型功能没有常驻帧要补）。
        let snap_hit = if self.config.screenshot.enabled {
            resolve_snap_hotkey(&self.config.screenshot, key, mods)
        } else {
            None
        };
        let rec_hk = self.config.record.hotkey.clone();
        let region_hk = self.config.record.region_hotkey.clone();
        let rec_trim_hk = self.config.record.trim_hotkey.clone();
        let gif_hk = self.config.gif.hotkey.clone();
        let trim_hk = self.config.gif.trim_hotkey.clone();
        match snap_hit {
            Some(SnapHotkey::Shot) => {
                self.begin_snap(snap::SnapMode::Shot);
                return;
            }
            Some(SnapHotkey::ShotPin) => {
                self.begin_snap(snap::SnapMode::ShotPin);
                return;
            }
            Some(SnapHotkey::ShotEdit { pin }) => {
                self.begin_snap(snap::SnapMode::ShotEdit { pin });
                return;
            }
            Some(SnapHotkey::PinClipboard) => {
                self.pin_next();
                return;
            }
            Some(SnapHotkey::Scroll) => {
                // ★ 一键开关（同录屏）：空闲 = 进框选；采集进行中 = 结束并交付。
                self.toggle_scroll(snap::SnapMode::Scroll);
                return;
            }
            Some(SnapHotkey::ScrollWide) => {
                // ★ 宽截图：与长截图共用同一套会话状态机，只有空闲分支的框选模式不同
                //   （方向在那一步定下）。会话已挂时两把键等价（开始/结束）。
                self.toggle_scroll(snap::SnapMode::ScrollWide);
                return;
            }
            None => {}
        }
        // ★ 录制快捷键 = **一键开关**（2026-09-19 用户口径）：空闲按下 → 3 秒倒计时后开录；
        //   倒计时中按下 → 取消；录制中按下 → 停止并落盘。MP4 与 GIF 各有一把这样的键，
        //   差别只在"归零后录哪种格式"。任一把在录制中按下都会停掉当前这一段 —— 一次只可能
        //   有一个录制在跑（同一条 `RecOptions` 通道、同一个倒计时），所以不存在"按 A 停了 B"
        //   的歧义；真正制造歧义的是**两把功能重叠的键**（旧的开始键 + 停止键），已删。
        if !rec_hk.is_empty() && self.config.record.enabled && hk_matches(&rec_hk, key, mods) {
            self.toggle_recording(RecKind::Mp4, None);
            return;
        }
        if !gif_hk.is_empty() && self.config.gif.enabled && hk_matches(&gif_hk, key, mods) {
            self.begin_gif_selection();
            return;
        }
        // ★ 选区录屏快捷键：复用 GIF 选框，框好后直接倒计时录这块区域（MP4）。
        //   与整屏录屏（`rec_hk`）并存：两把键各录各的，互不冲突。同样受 `record.enabled`
        //   管辖（关掉「视频录制」，选区录屏键一并失效）。
        if !region_hk.is_empty() && self.config.record.enabled && hk_matches(&region_hk, key, mods)
        {
            self.begin_mp4_region();
            return;
        }
        // ★ MP4 时间裁剪快捷键：打开最近一次录像的裁剪窗（没录过只提示）。受
        //   `record.enabled` 管辖，与 GIF 裁剪键（`trim_hk`）各管各的格式。
        if !rec_trim_hk.is_empty()
            && self.config.record.enabled
            && hk_matches(&rec_trim_hk, key, mods)
        {
            log::info!("MP4 时间裁剪快捷键命中");
            self.request_mp4_trim_standalone();
            return;
        }
        // ★ 裁剪快捷键排在最后：它跟录制状态**完全无关**（只开一个编辑窗），所以不需要像
        //   GIF 快捷键那样先看"是不是正在录"。放在后面也让"两把键恰好配成同一个组合"这种
        //   误配置退化成"先按录制语义走"—— 录制键的语义更重，优先它更合理。
        //   （同样受 `gif.enabled` 管辖：关掉「GIF录制」，裁剪键一并失效。）
        if !trim_hk.is_empty() && self.config.gif.enabled && hk_matches(&trim_hk, key, mods) {
            self.request_gif_trim_standalone();
            return;
        }

        // ★ 计时器快捷键：开始 / 停止一键开关（与录屏同款语义）。**快捷键为空即停用**，
        //   外加卡标题栏的功能开关——两道闸任一关上就不响应。
        let tm_hk = self.config.global.timer.hotkey.clone();
        if !tm_hk.is_empty() && self.config.global.timer.enabled && hk_matches(&tm_hk, key, mods) {
            self.toggle_timer();
            return;
        }

        // ★ 全屏放大：Esc 排在快捷键**之前**——它是"挡住一切"的状态，此时 Esc 必须
        //   先用来脱身（连带 Ctrl+Esc 这类系统组合不拦，只拦裸 Esc）。
        //   ★★ 这条不是可选项：全屏窗置顶且不透明，会把 CurSeen 自己的控制面板一起
        //     盖住；快捷键万一被改坏或冲突，Esc 是唯一还能出来的路。
        if self.config.fullscreen_running()
            && key == KeyCode::Esc
            && !(mods.ctrl || mods.alt || mods.shift || mods.win)
        {
            self.set_fullscreen_active(false);
            return;
        }
        // ★ 两把放大镜的启停快捷键（一键开关，切**运行态**）：只在**已载入**（面板勾选）
        //   时生效——2026-09-26 用户口径"勾选 = 允许开启，激活/退出只走快捷键"。
        //   全屏排在画中画之前：万一两把键配成同一个组合，语义重的全屏优先。
        if self.config.magnifier_fullscreen.enabled {
            let fs_hk = self.config.magnifier_fullscreen.hotkey.clone();
            if !fs_hk.is_empty() && hk_matches(&fs_hk, key, mods) {
                self.toggle_fullscreen_zoom();
                return;
            }
        }
        if self.config.magnifier_pip.enabled {
            let pip_hk = self.config.magnifier_pip.hotkey.clone();
            if !pip_hk.is_empty() && hk_matches(&pip_hk, key, mods) {
                self.toggle_pip_zoom();
            }
        }
        // （缩放不再有键盘快捷键：2026-09-21 夜起为内置 Ctrl+Win+滚轮，
        //  见 `platform::input::mouse` 的 WM_MOUSEWHEEL 注释与 `zoom_step`。）
    }
}

/// 该键码是不是"只有显示语义"的鼠标键（左/右/中/侧键）。
///
/// ★★ 它们**没有键盘虚拟键**：`platform::input::vk::to_vk_ext` 里没有鼠标键的分支，
/// 所以 [`crate::platform::input::inject::send_key`] 对它们必然返回 `false`。
/// 点键帽那条路必须靠这个函数分流——否则"注入失败"会退化成"点击被吞、键帽不亮、
/// 下层也收不到"三头落空（2026-09-26 用户报的"鼠标键点了没反应"）。
///
/// 滚轮（`WheelUp`/`WheelDown`）不在此列：它不是"按住型"的键，没有与之配对的抬起。
pub(crate) fn is_mouse_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::MouseLeft
            | KeyCode::MouseRight
            | KeyCode::MouseMiddle
            | KeyCode::MouseX1
            | KeyCode::MouseX2
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三把截图 / 贴图快捷键的**真相表**。
    ///
    /// 这张表是 [`resolve_snap_hotkey`] 的全部契约，也是 2026-09-23 那两条用户反馈的判据
    /// （"截图按住 Ctrl 能编辑、截图并贴图按住 Ctrl 没效果"）。三组配置各测一遍：
    /// 典型配置、存量（含 Ctrl 的旧默认值）、撞弦。
    /// ★ 2026-09-26 起出厂快捷键全留空（空 = 全不命中），所以 ① 显式摆一把典型组合，
    ///   语义与"出厂给 Alt+S / Alt+Q"的年代逐字节相同。
    #[test]
    fn snap_hotkey_truth_table() {
        let m = |ctrl: bool, alt: bool| Modifiers {
            ctrl,
            alt,
            shift: false,
            win: false,
        };

        // ── ① 典型配置：两把键各自的两条路都在 ──
        let mut s = Config::default().screenshot;
        s.hotkey = "Alt+S".into();
        s.hotkey_pin = "Alt+Q".into();
        s.hotkey_pin_clipboard = "Ctrl+Alt+P".into();
        let hit = |key| resolve_snap_hotkey(&s, key, m(false, true));
        let hit_c = |key| resolve_snap_hotkey(&s, key, m(true, true));
        assert_eq!(hit(KeyCode::Char('s')), Some(SnapHotkey::Shot));
        assert_eq!(hit(KeyCode::Char('q')), Some(SnapHotkey::ShotPin));
        assert_eq!(
            hit_c(KeyCode::Char('s')),
            Some(SnapHotkey::ShotEdit { pin: false }),
            "截图 + Ctrl = 编辑态、确认后不贴"
        );
        assert_eq!(
            hit_c(KeyCode::Char('q')),
            Some(SnapHotkey::ShotEdit { pin: true }),
            "截图并贴图 + Ctrl = 编辑态、确认后贴 —— 同一个 ShotEdit，只在 pin 上分岔"
        );
        assert_eq!(
            resolve_snap_hotkey(&s, KeyCode::Char('p'), m(true, true)),
            Some(SnapHotkey::PinClipboard)
        );
        // 修饰键必须齐：`Alt+S` 少了 Alt 就不该命中（防止误把裸 S 当快捷键）。
        assert_eq!(
            resolve_snap_hotkey(&s, KeyCode::Char('s'), m(false, false)),
            None
        );

        // ── ② 存量配置（hotkey_pin 还是含 Ctrl 的旧默认值）：本体优先，行为一点不变 ──
        let mut old = Config::default().screenshot;
        old.hotkey_pin = "Ctrl+Alt+Q".into();
        assert_eq!(
            resolve_snap_hotkey(&old, KeyCode::Char('q'), m(true, true)),
            Some(SnapHotkey::ShotPin),
            "规格已含 Ctrl ⇒ 拼不出变体 ⇒ 按住 Ctrl 仍是纯截图并贴图（不打扰老用户）"
        );
        assert!(
            crate::core::hotkeys::with_ctrl(&old.hotkey_pin).is_empty(),
            "含 Ctrl 的规格没有 Ctrl 变体，这正是 Config::snap_keys_missing_ctrl_variant 报的那件事"
        );

        // ── ③ 撞弦配置：hotkey 正好是 hotkey_pin 的 Ctrl 变体 ──
        let mut clash = Config::default().screenshot;
        clash.hotkey = "Ctrl+F3".into();
        clash.hotkey_pin = "F3".into();
        assert_eq!(
            resolve_snap_hotkey(&clash, KeyCode::F(3), m(true, false)),
            Some(SnapHotkey::Shot),
            "Ctrl+F3 是配置里写明的截图键 —— 本体优先于别人的变体，不静默改语义"
        );
        assert_eq!(
            resolve_snap_hotkey(&clash, KeyCode::F(3), m(false, false)),
            Some(SnapHotkey::ShotPin)
        );
        assert!(
            crate::core::hotkeys::with_ctrl(&clash.hotkey).is_empty(),
            "撞弦之所以成为死锁：截图键自己含 Ctrl，它也没有变体"
        );
    }

    /// ★★ 进编辑态的**两个入口**必须都在（2026-09-23 用户报的功能回归）。
    ///
    /// ① 快捷键那一刻按着 Ctrl（`resolve_snap_hotkey` 直接给出 `ShotEdit`）；
    /// ② **框选阶段**按住 Ctrl（`Shot` / `ShotPin` + 此刻 ctrl）。
    ///
    /// 入口 ② 是这条测试存在的全部理由。用户原话：*"我使用快捷键触发后，就是只需要鼠标
    /// 左键进行选区。但是我按住 Ctrl 却无法实现选区后的调整，那就是错的。这个和我用什么
    /// 快捷键不应该有关系。"* 而他用的快捷键是 `Ctrl+F3`（**本身含 Ctrl**）—— 那种键
    /// 拼不出 Ctrl 变体，入口 ① 天然够不着，只有 ② 能救。
    #[test]
    fn shot_edit_entry_truth_table() {
        use snap::SnapMode;
        // ① 模式已经定成编辑态：原样采用它的 `pin`，**不重判** Ctrl
        //    —— 否则"Ctrl + 截图并贴图"在用户松开 Ctrl 后会退化成不贴图。
        assert_eq!(
            shot_edit_entry(SnapMode::ShotEdit { pin: false }, false),
            Some(false)
        );
        assert_eq!(
            shot_edit_entry(SnapMode::ShotEdit { pin: true }, false),
            Some(true),
            "Ctrl 已松开也必须保持贴图 —— 按下那一刻的意图不能被松手推翻"
        );
        assert_eq!(
            shot_edit_entry(SnapMode::ShotEdit { pin: true }, true),
            Some(true)
        );
        // ② 普通截图 / 截图并贴图：只在"此刻按着 Ctrl"时升级，pin 随之继承。
        assert_eq!(
            shot_edit_entry(SnapMode::Shot, false),
            None,
            "没按 Ctrl ⇒ 老样子，直接进剪贴板"
        );
        assert_eq!(
            shot_edit_entry(SnapMode::Shot, true),
            Some(false),
            "拖框时按住 Ctrl ⇒ 进编辑态、确认后不贴图"
        );
        assert_eq!(
            shot_edit_entry(SnapMode::ShotPin, false),
            None,
            "没按 Ctrl ⇒ 老样子，直接贴图"
        );
        assert_eq!(
            shot_edit_entry(SnapMode::ShotPin, true),
            Some(true),
            "拖框时按住 Ctrl ⇒ 进编辑态、确认后贴图"
        );
        // ③ GIF 选框不是截图，任何情况下都不该掉进编辑态。
        assert_eq!(shot_edit_entry(SnapMode::Gif, false), None);
        assert_eq!(
            shot_edit_entry(SnapMode::Gif, true),
            None,
            "按住 Ctrl 框 GIF 区域也不能变成编辑态 —— 那个界面只有取消，进去就出不来"
        );
    }

    /// ★ 用户那个"撞弦"配置（`hotkey = Ctrl+F3` / `hotkey_pin = F3`）在**两个入口**下
    /// 都能进编辑态 —— 正是他 2026-09-23 报"连我们原来已经实现的功能都破坏了"的场景。
    ///
    /// 这条测试的价值在于把"配置长什么样"和"用户怎么按"绑在一起：
    /// 前一条真值表只证明了函数行为，这条证明**用户的实际按键路径**能走通。
    #[test]
    fn clashing_config_reaches_edit_mode_through_both_entries() {
        use snap::SnapMode;
        let mut s = Config::default().screenshot;
        s.hotkey = "Ctrl+F3".into();
        s.hotkey_pin = "F3".into();
        let m = |ctrl: bool| Modifiers {
            ctrl,
            alt: false,
            shift: false,
            win: false,
        };

        // 按 Ctrl+F3：本体命中 ⇒ 普通截图模式（本体优先，语义不静默改变）；
        // 但按住它时 Ctrl 全程按着 ⇒ 入口 ② 照样把这次框选送进编辑态。
        assert_eq!(
            resolve_snap_hotkey(&s, KeyCode::F(3), m(true)),
            Some(SnapHotkey::Shot)
        );
        assert_eq!(
            shot_edit_entry(SnapMode::Shot, true),
            Some(false),
            "Ctrl+F3 拖框（Ctrl 全程按着）必须进编辑态 —— 这就是用户报的回归"
        );

        // 按 F3（截图并贴图）：同样靠入口 ② 进编辑态，pin 保持 true。
        assert_eq!(
            resolve_snap_hotkey(&s, KeyCode::F(3), m(false)),
            Some(SnapHotkey::ShotPin)
        );
        assert_eq!(
            shot_edit_entry(SnapMode::ShotPin, true),
            Some(true),
            "F3 拖框时按住 Ctrl ⇒ 编辑态 + 确认后贴图（用户要的正是这个）"
        );
    }

    /// 回归测试（2026-09-18）：虚拟键盘「捕获点击」（启用且未勾鼠标穿透）时，鼠标输入门
    /// 必须打开。
    ///
    /// 这是当天修掉的静默回归：`mouse_input_needed()` 原本只查拖尾/击键回显/绘制，**漏了
    /// 虚拟键盘**。结果「只开键盘、关掉其余」时，桥接线程在 `MOUSE_INPUT_NEEDED=false` 下
    /// 把鼠标事件整条丢弃（`spawn_input_bridge` 的 `continue`），`feed` 收不到 `MouseDown`，
    /// 点键帽一律不注入——表现就是"鼠标穿透/键盘点按失效"，且没有任何报错。
    #[test]
    fn mouse_input_needed_opens_gate_for_vk_capture() {
        // 仅启用虚拟键盘、其余三项全关，且未勾「鼠标穿透」（即捕获点击 → 必须收鼠标）。
        let mut cfg = Config::default();
        cfg.virtual_keyboard.enabled = true;
        cfg.virtual_keyboard.click_through = false;
        let rt = Runtime::new(cfg);
        assert!(
            rt.mouse_input_needed(),
            "仅开键盘且未勾穿透时鼠标输入门必须打开，否则点键帽不注入"
        );
    }

    /// 反向：勾了「鼠标穿透」（不捕获点击）时，键盘不应打开鼠标门——穿透态下命中区为空，
    /// 钩子天然放行，没必要无谓地唤醒 1000 次/秒的桥接通道。
    #[test]
    fn mouse_input_needed_closed_when_vk_passthrough() {
        let mut cfg = Config::default();
        cfg.virtual_keyboard.enabled = true;
        cfg.virtual_keyboard.click_through = true;
        let rt = Runtime::new(cfg);
        assert!(
            !rt.mouse_input_needed(),
            "勾了鼠标穿透时不应靠键盘打开鼠标门"
        );
    }

    /// ★ 卡键兜底：`vk_release_all` 必须幂等 —— 没有按住的键时它什么都不做。
    ///
    /// 它的两个调用点（功能被关掉、进程退出）**不保证**当时真有键按着：进程退出路径
    /// 更是每次都走。所以"空手调用"必须是安全的；一旦它开始无条件发 `SendInput`，
    /// 每次退出都会往系统输入队列里塞一条凭空的抬起 —— 更糟的是，如果有实现把它写成
    /// "抬起所有键"，那会**打断用户正在物理按住的键**。
    #[test]
    fn vk_release_all_is_safe_when_nothing_is_held() {
        let mut rt = Runtime::new(Config::default());
        assert!(rt.vk_pointer.is_none(), "新建的运行时不该有按住的虚拟键");
        rt.vk_release_all();
        rt.vk_release_all();
        assert!(rt.vk_pointer.is_none());
    }
}
