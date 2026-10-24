//! 按键弹窗控制器。逐分支对照 controller.py:247-326。

use std::collections::HashMap;

use crate::core::events::Modifiers;

use super::keymap::{self, KeyKind};
use super::keynames::Keynames;
use super::merge::{is_shortcut_line, KeyItem, KeyItemQueue};
use super::settings::{
    DisplayMode, KeyDisplaySettings, FADE_DURATION, MOD_STUCK_TIMEOUT, REMOVE_DELAY,
};

/// 击键回显控制器。
pub struct KeyController {
    pub items: KeyItemQueue,
    pub active: Option<KeyItem>,
    /// 修饰键按下状态。
    pub mods: HashMap<&'static str, bool>,
    /// 修饰键的按下时刻（用于 5 秒卡死清理）。
    pub mod_times: HashMap<&'static str, f64>,
    pub fade_duration: f32,
    pub remove_delay: f32,
    /// 淡出延迟（秒）。来自配置 `key_display.fade_delay`，由模块在同步设置时写入。
    pub fade_delay: f32,
    pub mod_stuck: f32,
    /// 键名覆盖表（`assets/KeyDisplay/keynames.ini`，按键频率热重载）。
    pub keynames: Keynames,
    pub enabled: bool,
    pub dirty: bool,
}

impl KeyController {
    pub fn new(settings: &KeyDisplaySettings) -> Self {
        Self {
            items: KeyItemQueue::new(),
            active: None,
            mods: HashMap::new(),
            mod_times: HashMap::new(),
            fade_duration: FADE_DURATION,
            remove_delay: REMOVE_DELAY,
            fade_delay: settings.fade_delay,
            mod_stuck: MOD_STUCK_TIMEOUT,
            keynames: Keynames::new(crate::core::paths::keynames_ini_path()),
            enabled: true,
            dirty: false,
        }
    }

    /// 处理按键按下事件。九步，严格按此顺序。
    pub fn on_key_pressed(&mut self, vk: u16, now: f64, settings: &KeyDisplaySettings) {
        if !self.enabled {
            return;
        }
        // ★ 键名覆盖表（按键频率）：mtime 变了才重读 ini——改文件保存后按一次键即生效，
        //   不占每帧开销（文件没变时只付一次 metadata 调用）。
        self.keynames.refresh();
        // ② 清理卡死修饰键
        self.cleanup_stuck_mods(now);

        // ③ lookup
        let info = match keymap::lookup_vk(vk) {
            Some(info) => info,
            None => return,
        };

        // ④ 修饰键只更新状态，不生成条目
        if info.kind == KeyKind::Mod {
            self.mods.insert(info.display, true);
            self.mod_times.insert(info.display, now);
            return;
        }

        // ⑤ resolve key text
        let parts = self.resolve_key_text(vk, settings);
        if parts.is_empty() {
            return;
        }

        // ⑥ has_mod
        let has_mod = self.has_any_mod();

        // ⑦ display_mode 过滤
        match settings.display_mode {
            // 「排除鼠标」只排除鼠标条目，键盘语义同「全部」。
            DisplayMode::NoMouse | DisplayMode::All => {}
            DisplayMode::ShortcutsFun => {
                if !has_mod && !info.is_fun {
                    return;
                }
            }
            DisplayMode::Shortcuts => {
                if !has_mod {
                    return;
                }
            }
        }

        // ⑧ display & is_char
        let display = if has_mod {
            parts.concat()
        } else {
            parts[0].clone()
        };
        let is_char = !has_mod && display.chars().count() == 1 && display.is_ascii();

        // ⑨ 弹窗逻辑（A-E 五条分支，顺序不可换）：按键与鼠标按键**合流**到
        //   [`Self::push_display`] 这一个出口——两条输入源的弹窗行为因此逐字一致。
        self.push_display(
            display,
            is_char,
            has_mod,
            settings.merge_interval as f64,
            now,
        );
    }

    /// 鼠标按键按下（左键 / 右键 / 中键 / 滚轮上·下·左·右 / 侧键1·2）。
    ///
    /// 与按键合流到同一条 [`Self::push_display`]——排队 / 合并 / 淡出 / 修饰键前缀
    /// 与按键完全一致，不另写一套。
    ///
    /// ★ 「显示哪些键」的模式过滤对鼠标**同规则适用**：普通点击不是快捷键，
    ///   「仅快捷键 / 快捷键+功能键」模式下只有带修饰键的鼠标组合（Ctrl + 左键）
    ///   才进弹窗——否则受限模式会被普通左键刷屏，模式名存实亡。
    pub fn on_mouse_pressed(
        &mut self,
        id: &str,
        builtin: &'static str,
        now: f64,
        settings: &KeyDisplaySettings,
    ) {
        if !self.enabled {
            return;
        }
        // 键名覆盖表与键盘同频检查：纯鼠标用户改 ini 也能按一次即生效。
        self.keynames.refresh();
        // ★ ini 覆盖优先：`assets/KeyDisplay/keynames.ini` 鼠标节（MouseLeft 等 9 条）
        //   改文本即生效；没写的键用内置名。
        // 先落成 String：text_for_id 借用 self.keynames，不能跨过下面的
        // cleanup_stuck_mods（可变借用）存活。
        let name: String = self.keynames.text_for_id(id).unwrap_or(builtin).to_string();
        // 与 on_key_pressed 同款：先清掉超时的卡死修饰键（鬼影不再进前缀）。
        self.cleanup_stuck_mods(now);
        let has_mod = self.has_any_mod();
        match settings.display_mode {
            // ★「排除鼠标」一票否决：该模式下鼠标条目（含带修饰键的组合）全部不显示。
            DisplayMode::NoMouse => return,
            DisplayMode::Shortcuts | DisplayMode::ShortcutsFun => {
                if !has_mod {
                    return;
                }
            }
            DisplayMode::All => {}
        }
        let display = if has_mod {
            // 前缀与 resolve_key_text 同源：按住的 Ctrl/Alt/Shift/Win/Menu 在前。
            let mut parts: Vec<String> = Vec::new();
            for &m in &["Ctrl", "Alt", "Shift", "Win", "Menu"] {
                if self.mods.get(m) == Some(&true) {
                    parts.push(m.to_string());
                }
            }
            parts.push(name.clone());
            parts.join(" + ")
        } else {
            name
        };
        self.push_display(display, false, has_mod, settings.merge_interval as f64, now);
    }

    /// ⑨ 弹窗逻辑（A-E 五条分支，顺序不可换）的公共出口。
    ///
    /// 按键（[`Self::on_key_pressed`]）与鼠标按键（[`Self::on_mouse_pressed`]）都走到
    /// 这里。合流而不是各写一份，是为了"点了鼠标之后弹窗怎么排队/合并/淡出"与按键
    /// 完全一致——否则用户会觉得鼠标那一路是另一套东西。
    fn push_display(
        &mut self,
        display: String,
        is_char: bool,
        has_mod: bool,
        merge_interval: f64,
        now: f64,
    ) {
        match &self.active {
            // A: 上一个条目已经在淡出中
            Some(a) if a.opacity < 1.0 => {
                let new_item = KeyItem::new(&display, now, is_char);
                let old_active = self.active.take();
                self.active = Some(new_item);
                if let Some(old) = old_active {
                    self.items.push_back(old);
                }
            }
            // B: has_mod && active.is_some()
            Some(a) if has_mod => {
                let last_raw = &a.groups[0].text;
                let dt = now - a.timestamp;
                let matches = last_raw == &display
                    || last_raw.trim_start() == display.as_str()
                    || *last_raw == format!(" {}", display);
                if matches && dt < merge_interval && !a.is_fading {
                    if let Some(ref mut active) = self.active {
                        active.merge(&KeyItem::new(&display, now, false));
                        active.timestamp = now;
                        active.opacity = 1.0;
                    }
                } else {
                    let new_item = KeyItem::new(&display, now, is_char);
                    let old_active = self.active.take();
                    self.active = Some(new_item);
                    if let Some(old) = old_active {
                        self.items.push_back(old);
                    }
                }
            }
            // C: 上一条是快捷键
            Some(a) if is_shortcut_line(&a.display_text()) => {
                let new_item = KeyItem::new(&display, now, is_char);
                let old_active = self.active.take();
                self.active = Some(new_item);
                if let Some(old) = old_active {
                    self.items.push_back(old);
                }
            }
            // D: active.is_some()
            Some(a) => {
                let dt = now - a.timestamp;
                if dt < merge_interval {
                    if let Some(ref mut active) = self.active {
                        active.merge(&KeyItem::new(&display, now, is_char));
                        active.timestamp = now;
                        active.opacity = 1.0;
                        active.is_fading = false;
                    }
                } else {
                    let new_item = KeyItem::new(&display, now, is_char);
                    let old_active = self.active.take();
                    self.active = Some(new_item);
                    if let Some(old) = old_active {
                        self.items.push_back(old);
                    }
                }
            }
            // E: active 为空
            None => {
                self.active = Some(KeyItem::new(&display, now, is_char));
            }
        }

        self.dirty = true;
    }

    /// 处理按键释放事件。
    pub fn on_key_released(&mut self, vk: u16) {
        if let Some(info) = keymap::lookup_vk(vk) {
            if info.kind == KeyKind::Mod {
                self.mods.insert(info.display, false);
                self.mod_times.remove(info.display);
            }
        }
    }

    /// 用事件自带的**权威**修饰键状态校正本地 `mods`。
    ///
    /// # 为什么需要
    ///
    /// 本地 `mods` 只由"修饰键自己的 KeyDown / KeyUp"维护，而一次"按住再松开"是**两条**
    /// 事件。任何一条没到（高速输入下钩子丢事件、程序在被按住时启动、注入的合成键不经过
    /// 钩子），就会留下一个**幽灵修饰键**。它的两个可见后果正是用户报的现象：
    ///
    /// 1. 单个字母被显示成「Shift + 字母」；
    /// 2. 因为 `has_mod` 走的是分支 B（每次不匹配就另起条目，而不是在同一个弹窗里累加），
    ///    **每个字母都单独一个小窗**。
    ///
    /// 钩子侧的 `Modifiers` 是从它自己的原子缓存算的，**每条事件都携带**，因此自校正：
    /// 下一个不带 Shift 的按键到来时 `shift == false`，幽灵当场被清掉 —— 不必等
    /// `MOD_STUCK_TIMEOUT`（5 秒）兜底，也不必依赖 KeyUp 一定送达。
    ///
    /// ⚠️ 只覆盖 `Modifiers` 里有的四个；`Menu`（菜单键）不在其中，仍走
    /// [`on_key_released`](Self::on_key_released)。
    pub fn sync_mods(&mut self, mods: &Modifiers, _now: f64) {
        for (name, pressed) in [
            ("Ctrl", mods.ctrl),
            ("Alt", mods.alt),
            ("Shift", mods.shift),
            ("Win", mods.win),
        ] {
            self.mods.insert(name, pressed);
            if !pressed {
                self.mod_times.remove(name);
            }
        }
    }

    /// 清空全部状态（功能关闭时调用）。
    ///
    /// 不这么做的话，"关掉击键回显、开着拖尾"期间按键照记 —— 重新打开时会看到一堆
    /// 几分钟前按过的键。与虚拟键盘 `tick` 里关掉即清 `press_at` / `held` 同款。
    pub fn clear(&mut self) {
        self.items = KeyItemQueue::new();
        self.active = None;
        self.mods.clear();
        self.mod_times.clear();
    }

    /// 帧推进。三段淡出。
    ///
    /// ★ 时间单位是**秒**（与 Python `controller.py` 的 `time.time()` 一致）：`now` 由
    /// `KeyDisplayModule` 从挂钟换算后传入，`fade_delay` / `fade_duration` / `remove_delay`
    /// / `mod_stuck` 全是秒。此前调用方传的是**毫秒**，于是 `age >= 10.0`(秒) 要等 10 秒，
    /// 而 `fa < 0.5`(秒) 对毫秒而言立即成立 → 弹窗出生约 10ms 就被移除，屏上几乎看不见。
    pub fn tick(&mut self, _dt: f32, now: f64) -> bool {
        self.cleanup_stuck_mods(now);

        let fade_delay = self.fade_delay as f64;
        let mut changed = false;

        // 推进 items
        let mut to_remove = Vec::new();
        for (i, item) in self.items.items_mut().enumerate() {
            let age = now - item.timestamp;
            if !item.is_fading && age >= fade_delay {
                item.is_fading = true;
            }
            if item.is_fading {
                let fa = age - fade_delay;
                if fa < self.fade_duration as f64 {
                    item.opacity = (1.0 - fa as f32 / self.fade_duration).max(0.0);
                    changed = true;
                } else {
                    let ra = fa - self.fade_duration as f64;
                    if ra >= self.remove_delay as f64 {
                        to_remove.push(i);
                    }
                }
            }
        }
        // 从后往前删除
        for &i in to_remove.iter().rev() {
            self.items.remove(i);
            changed = true;
        }

        // 推进 active
        if let Some(ref mut active) = self.active {
            let age = now - active.timestamp;
            if !active.is_fading && age >= fade_delay {
                active.is_fading = true;
            }
            if active.is_fading {
                let fa = age - fade_delay;
                if fa < self.fade_duration as f64 {
                    active.opacity = (1.0 - fa as f32 / self.fade_duration).max(0.0);
                    changed = true;
                } else {
                    let ra = fa - self.fade_duration as f64;
                    if ra >= self.remove_delay as f64 {
                        self.active = None;
                        changed = true;
                    }
                }
            }
        }

        if changed {
            self.dirty = true;
        }
        changed
    }

    /// 下一次画面会变的时刻距现在多少毫秒（供帧驱动排程）。
    ///
    /// - `None`：没有任何弹窗，完全静止（可以让 UI 线程睡到底）。
    /// - `Some(0)`：正在淡出，需要连续帧。
    /// - `Some(ms)`：弹窗还静止着，`ms` 毫秒后才开始淡出 —— 到点再唤醒即可。
    ///
    /// 没有这个查询就只能"要么一直续帧、要么永不续帧"：前者空转烧 CPU，后者弹窗会
    /// 永远停在屏幕上（分层窗帧间不清缓冲，最后一帧就是最终可见画面）。
    /// `now` 单位同 [`tick`](Self::tick)，是秒。
    pub fn next_change_ms(&self, now: f64) -> Option<u32> {
        if self.items.is_empty() && self.active.is_none() {
            return None;
        }
        let delay = self.fade_delay as f64;
        let mut best = f64::INFINITY;
        let mut consider = |timestamp: f64, fading: bool| {
            if fading {
                best = 0.0;
                return;
            }
            best = best.min((delay - (now - timestamp)).max(0.0));
        };
        for item in self.items.items() {
            consider(item.timestamp, item.is_fading);
        }
        if let Some(active) = &self.active {
            consider(active.timestamp, active.is_fading);
        }
        // 已经在淡出（best == 0）时不必转毫秒再截断；否则向上取整，宁可早醒不可晚醒。
        Some((best * 1000.0).ceil() as u32)
    }

    fn cleanup_stuck_mods(&mut self, now: f64) {
        let mut stuck = Vec::new();
        for (&k, &t) in &self.mod_times {
            if now - t > self.mod_stuck as f64 {
                stuck.push(k);
            }
        }
        for k in stuck {
            self.mods.remove(k);
            self.mod_times.remove(k);
        }
    }

    fn has_any_mod(&self) -> bool {
        ["Ctrl", "Alt", "Shift", "Win", "Menu"]
            .iter()
            .any(|&m| self.mods.get(m) == Some(&true))
    }

    fn resolve_key_text(&mut self, vk: u16, _settings: &KeyDisplaySettings) -> Vec<String> {
        let mut parts = Vec::new();
        for &m in &["Ctrl", "Alt", "Shift", "Win", "Menu"] {
            if self.mods.get(m) == Some(&true) {
                // 修饰键前缀同样过覆盖表：ini 里 `Ctrl = Ctrl键` 改名即变；
                // 内部状态仍用内置名（has_any_mod / 卡死清理不受改名影响）。
                parts.push(self.keynames.mod_text(m).to_string());
            }
        }
        let info = match keymap::lookup_vk(vk) {
            Some(info) => info,
            None => return Vec::new(),
        };
        // ★ ini 覆盖优先：`assets/KeyDisplay/keynames.ini` 里写 `Space = 空格`
        //   就显示"空格"。改过名的键若不再是内置功能键名，绘制色回退普通键色
        //   （render 的 display_info 查不到 → font_color），ini 头注释写明了。
        let mut display = match self.keynames.text_for(vk) {
            Some(text) => text.to_string(),
            None => info.display.to_string(),
        };
        if !parts.is_empty() && display.chars().count() == 1 {
            display = display.to_uppercase();
        }
        if parts.is_empty() {
            vec![display]
        } else {
            let mut result = Vec::new();
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    result.push(" + ".to_string());
                }
                result.push(p.clone());
            }
            result.push(" + ".to_string());
            result.push(display);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_settings() -> KeyDisplaySettings {
        KeyDisplaySettings::default()
    }

    #[test]
    fn new_controller() {
        let c = KeyController::new(&make_settings());
        assert!(c.enabled);
        assert!(c.items.is_empty());
        assert!(c.active.is_none());
    }

    /// ★ 「排除鼠标」模式（2026-09-22 新增分类）：鼠标条目一票否决——带修饰键的
    ///   鼠标组合也不显示；键盘不受牵连（语义同「全部」）。
    #[test]
    fn no_mouse_mode_suppresses_mouse_but_not_keyboard() {
        let mut c = KeyController::new(&make_settings());
        c.enabled = true;
        let st = KeyDisplaySettings {
            display_mode: DisplayMode::NoMouse,
            ..make_settings()
        };
        c.keynames.refresh();

        // 鼠标：无修饰键 → 被模式过滤；带修饰键也 → 一票否决。
        c.on_mouse_pressed("MouseLeft", "左键", 1.0, &st);
        assert!(c.active.is_none(), "排除鼠标模式下普通左键不应显示");
        c.mods.insert("Ctrl", true);
        c.on_mouse_pressed("MouseLeft", "左键", 2.0, &st);
        assert!(c.active.is_none(), "排除鼠标模式下带修饰键的鼠标也不应显示");
        c.mods.clear();

        // 键盘：语义同「全部」。
        c.on_key_pressed(0x41, 3.0, &st);
        assert!(c.active.is_some(), "排除鼠标模式不影响键盘");
    }

    #[test]
    fn char_key_creates_item() {
        let mut c = KeyController::new(&make_settings());
        let vk = 0x41; // A
        c.on_key_pressed(vk, 1.0, &make_settings());
        assert!(c.active.is_some());
        assert_eq!(c.active.as_ref().unwrap().display_text(), "A");
    }

    /// ★ 时间单位回归锁 + 帧排程。`now` 是**秒**：弹窗要在 `fade_delay`(默认 10s) 后开始淡出、
    /// 用 `fade_duration`(0.5s) 淡完、再挂 `remove_delay`(1s) 才移除。
    ///
    /// 此前调用方传毫秒，这三个常量却按秒解释 → 弹窗出生约 10ms 就被移除，屏上根本看不见。
    #[test]
    fn popup_lingers_then_fades_then_is_scheduled_away() {
        let settings = make_settings(); // fade_delay = 10.0 秒
        let mut c = KeyController::new(&settings);
        c.on_key_pressed(0x41, 0.0, &settings);

        // 静止期：画面还是纯不透明，但要**约好** fade_delay 之后唤醒。
        let ms = c.next_change_ms(0.0).expect("有弹窗就必须有排程");
        assert!(
            (9_900..=10_100).contains(&ms),
            "剩余等待应≈10s，实际 {ms}ms"
        );
        assert_eq!(c.active.as_ref().map(|a| a.opacity), Some(1.0));

        // 到达淡出时刻：转为连续帧，不透明度下降。
        c.tick(0.016, 10.2);
        let opacity = c.active.as_ref().map(|a| a.opacity).unwrap_or(1.0);
        assert!(opacity < 1.0, "淡出中不透明度应下降，实际 {opacity}");
        assert_eq!(c.next_change_ms(10.2), Some(0), "淡出期间要连续帧");

        // 淡出 0.5s + 移除延迟 1.0s 之后：彻底消失且不再排程。
        c.tick(0.016, 10.0 + 0.5 + 1.0 + 0.1);
        assert!(c.active.is_none(), "过了移除延迟应当清掉");
        assert_eq!(c.next_change_ms(11.6), None, "没有弹窗就不该再排程");
    }

    #[test]
    fn char_key_repeat_merges() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x41, 1.0, &settings);
        c.on_key_pressed(0x41, 1.5, &settings); // 出厂合并间隔已改 1.0，间隔压到 0.5s 内
        assert_eq!(c.active.as_ref().unwrap().display_text(), "A x2");
    }

    #[test]
    fn ctrl_c_creates_shortcut() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings); // Ctrl
        c.on_key_pressed(0x43, 2.0, &settings); // C
        let text = c.active.as_ref().unwrap().display_text();
        assert!(text.contains("Ctrl"));
        assert!(text.contains("C"));
    }

    #[test]
    fn ctrl_c_repeat_merges() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings);
        c.on_key_pressed(0x43, 1.6, &settings); // 出厂合并间隔已改 1.0：各步间隔压到 1s 内
        c.on_key_pressed(0x11, 2.0, &settings);
        c.on_key_pressed(0x43, 2.4, &settings);
        let text = c.active.as_ref().unwrap().display_text();
        assert!(text.contains("x2"), "徽标格式 = 名 空格 x 次数：{text}");
    }

    #[test]
    fn plain_key_does_not_merge_with_shortcut() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        // Ctrl + C
        c.on_key_pressed(0x11, 1.0, &settings);
        c.on_key_pressed(0x43, 2.0, &settings);
        // Release Ctrl, then A (plain key)
        c.on_key_released(0x11);
        c.on_key_pressed(0x41, 3.0, &settings);
        // 两条：active 是新条目，旧的被 push_back
        assert_eq!(c.items.len(), 1);
        let text = c.active.as_ref().unwrap().display_text();
        assert_eq!(text, "A");
    }

    #[test]
    fn ctrl_c_then_ctrl_v() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings);
        c.on_key_pressed(0x43, 2.0, &settings);
        c.on_key_pressed(0x11, 3.0, &settings);
        c.on_key_pressed(0x56, 4.0, &settings);
        assert_eq!(c.items.len(), 1);
        let text = c.active.as_ref().unwrap().display_text();
        assert!(text.contains("V"));
    }

    #[test]
    fn mod_release_clears_mod_state() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings); // Ctrl down
        assert!(c.has_any_mod());
        c.on_key_released(0x11); // Ctrl up
        assert!(!c.has_any_mod());
    }

    /// ★ 修饰键单独按下**不生成条目**（2026-10-24 定案）。
    ///
    /// 用户问过"Shift/Alt 要不要像普通键那样单键显示"。决定：不显示。单独按修饰键
    /// 不构成操作，回显要看的是快捷键结果；按住态另有虚拟键盘可视化。这里有两条断言：
    /// 单按不出条目、但状态确实记下了（紧接着的组合键要带上前缀）。
    #[test]
    fn a_modifier_alone_never_creates_an_item_but_still_prefixes_the_next_key() {
        let s = make_settings();
        let mut c = KeyController::new(&s);
        // Ctrl / Alt / Shift / LWin
        for vk in [0x11_u16, 0x12, 0x10, 0x5B] {
            c.on_key_pressed(vk, 1.0, &s);
            assert!(
                c.active.is_none(),
                "修饰键 {vk:#x} 单独按下不该生成条目（用户 2026-10-24 定案）"
            );
            c.on_key_released(vk);
        }
        // 状态仍然有效：Ctrl 按住 + C → 组合键带前缀。
        c.on_key_pressed(0x11, 2.0, &s);
        c.on_key_pressed(0x43, 3.0, &s);
        let text = c
            .active
            .as_ref()
            .map(KeyItem::display_text)
            .unwrap_or_default();
        assert_eq!(text, "Ctrl + C");
    }

    #[test]
    fn merge_interval_timeout() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x41, 1.0, &settings);
        c.on_key_pressed(0x41, 100.0, &settings); // 远超 merge_interval
        assert_eq!(c.items.len(), 1);
        let text = c.active.as_ref().unwrap().display_text();
        assert_eq!(text, "A");
    }

    #[test]
    fn char_key_after_shortcut_is_new_item() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings); // Ctrl
        c.on_key_pressed(0x43, 2.0, &settings); // C
                                                // Release Ctrl so A is not a shortcut
        c.on_key_released(0x11);
        c.on_key_pressed(0x41, 3.0, &settings); // A (plain)
        let text = c.active.as_ref().unwrap().display_text();
        assert_eq!(text, "A");
    }

    #[test]
    fn clear_shortcut_prefix_detection() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        // Create a shortcut: Ctrl + C
        c.on_key_pressed(0x11, 1.0, &settings);
        c.on_key_pressed(0x43, 2.0, &settings);
        let text = c.active.as_ref().unwrap().display_text();
        // Should detect as shortcut line
        assert!(is_shortcut_line(&text));
    }

    #[test]
    fn disabled_controller() {
        let mut c = KeyController::new(&make_settings());
        c.enabled = false;
        c.on_key_pressed(0x41, 1.0, &make_settings());
        assert!(c.active.is_none());
    }

    #[test]
    fn backspace_is_not_char() {
        let mut c = KeyController::new(&make_settings());
        c.on_key_pressed(0x08, 1.0, &make_settings());
        assert!(c.active.is_some());
        assert!(!c.active.as_ref().unwrap().is_char);
    }

    #[test]
    fn display_mode_shortcuts_only() {
        let mut settings = make_settings();
        settings.display_mode = DisplayMode::Shortcuts;
        let mut c = KeyController::new(&settings);
        c.on_key_pressed(0x41, 1.0, &settings); // A (no mod)
        assert!(c.active.is_none());
        c.on_key_pressed(0x11, 1.0, &settings); // Ctrl
        c.on_key_pressed(0x41, 2.0, &settings); // A (with mod)
        assert!(c.active.is_some());
    }

    #[test]
    fn stuck_mod_cleanup() {
        let mut c = KeyController::new(&make_settings());
        let settings = make_settings();
        c.on_key_pressed(0x11, 1.0, &settings); // Ctrl
                                                // 6 秒后（超过 5 秒超时）
        c.cleanup_stuck_mods(7.0);
        assert!(!c.has_any_mod());
    }

    /// ★ 幽灵修饰键：`Shift` 的 KeyUp 丢了之后，**下一个不带 Shift 的按键必须把它清掉**。
    ///
    /// 这是用户报「打英文单字符总是显示 Shift + 字母，且每个字母单独一个小窗」的根因锁。
    /// 成因：本地 `mods` 只由修饰键自己的 KeyDown / KeyUp 维护，键程是两条事件，丢一条就
    /// 留下一个"永远按着"的修饰键 → `has_mod` 恒真 → 走分支 B（不匹配就另起条目）→ 每个
    /// 字母一个新弹窗。修复靠 `sync_mods`（事件自带权威 `Modifiers`，自校正）。
    #[test]
    fn stale_shift_is_cleared_by_the_next_plain_letter() {
        let s = make_settings();
        let mut c = KeyController::new(&s);

        // 按住 Shift 打 A —— 正常（此刻 Shift 确实按着）。
        c.sync_mods(
            &Modifiers {
                shift: true,
                ..Default::default()
            },
            1.0,
        );
        c.on_key_pressed(0x41, 1.0, &s);
        assert_eq!(c.active.as_ref().unwrap().display_text(), "Shift + A");

        // 松开 Shift，但**它的 KeyUp 没送到**。下一个字母的事件里 shift=false —— 幽灵当场
        // 应该被清掉，而不是继续渲染成 "Shift + B"。
        c.sync_mods(&Modifiers::default(), 2.0);
        assert!(!c.has_any_mod(), "下一条事件就该清掉幽灵修饰键");
        assert_eq!(
            c.active.as_ref().unwrap().display_text(),
            "Shift + A",
            "清掉幽灵后不得再追加 Shift 前缀"
        );

        // 紧接着打字：新条目是**纯键 B**（上一条「Shift + A」是快捷键行，按分支 C 另起一条，
        // 这是设计如此），关键是它**不再带 Shift**。
        c.on_key_pressed(0x42, 2.0, &s);
        assert_eq!(c.items.len(), 1, "旧的快捷键行应被推进历史");
        assert_eq!(c.active.as_ref().unwrap().display_text(), "B");

        // 再打一个字母：必须**累加进同一个弹窗** —— 这就是「每个字母一个小窗」的反面。
        c.on_key_pressed(0x43, 2.4, &s);
        assert_eq!(c.items.len(), 1, "连续字母不得再各起一个条目");
        assert_eq!(c.active.as_ref().unwrap().display_text(), "B C");
    }

    /// 修饰键状态可以**只**来自事件（程序在用户按住 Ctrl 时启动 → 没有它的 KeyDown）。
    #[test]
    fn mod_state_can_come_from_the_event_alone() {
        let s = make_settings();
        let mut c = KeyController::new(&s);
        assert!(!c.has_any_mod());
        c.sync_mods(
            &Modifiers {
                ctrl: true,
                ..Default::default()
            },
            1.0,
        );
        assert!(c.has_any_mod());
        c.on_key_pressed(0x43, 1.0, &s); // C
        assert!(
            c.active.as_ref().unwrap().display_text().contains("Ctrl"),
            "实得 {}",
            c.active.as_ref().unwrap().display_text()
        );
    }

    /// 功能关闭时 `clear()` 必须把条目与修饰键状态一起清掉（不留下次打开时的"旧键"）。
    #[test]
    fn clear_drops_items_and_mod_state() {
        let s = make_settings();
        let mut c = KeyController::new(&s);
        c.on_key_pressed(0x41, 1.0, &s);
        c.sync_mods(
            &Modifiers {
                win: true,
                ..Default::default()
            },
            1.0,
        );
        assert!(!c.items.is_empty() || c.active.is_some());
        c.clear();
        assert!(c.items.is_empty() && c.active.is_none());
        assert!(!c.has_any_mod());
        assert!(c.next_change_ms(2.0).is_none(), "清空后不该再排帧");
    }
}
