//! 虚拟键盘模块：把按下的物理键映射到键盘布局上并高亮显示。
//!
//! # 与「击键回显」的分工
//!
//! | | 击键回显 | 虚拟键盘 |
//! |---|---|---|
//! | 回答的问题 | 刚才**打出了什么**（`Ctrl+C`） | 刚才**按的是哪个键位**（左 Ctrl 那颗 + C 那颗） |
//! | 形态 | 一串文本弹窗 | 一块常驻的键盘图 |
//! | 状态 | 历史列表 + 淡出 | 每个键位一个高亮计时 |
//!
//! 两者共用同一条输入通路（`Runtime::feed` → `tick`），互不干扰，可同时开。
//!
//! # 为什么高亮要"保持一段时间"而不是跟着 keydown/keydown 闪一下
//!
//! 一次快速敲击的按下-抬起间隔常常小于一帧（16ms），如果只在 `KeyDown` 到 `KeyUp`
//! 之间高亮，用户**什么也看不见**。故按下后至少亮 `hold` 秒（默认 0.1s，范围 0.0~2.0），
//! 之后再淡出 [`fade_sec`]（跟随 `hold`、上限 [`FADE_SEC`]）——`hold = 0` 就是松开即灭。
//! 修饰键（`Ctrl` / `Shift` / `Alt`）只要还按着就一直亮——它们本来就是按住才有用的键。

pub mod defaults;
pub mod layout;
pub mod vk_config;

use std::collections::{HashMap, HashSet};

use self::layout::KeyDef;
use crate::core::config::{Config, VirtualKeyboardSettings};
use crate::core::draw::DrawList;
use crate::core::events::{InputEvent, KeyCode};
use crate::core::geometry::Rect;
use crate::core::tick::Tick;
use crate::modules::Module;
use crate::platform::display::ScreenRect;

/// 高亮淡出时长的**上限**（秒）。`hold` 走完之后再淡这么久，想立刻灭就把 `hold` 调到 0。
pub const FADE_SEC: f32 = 0.25;

/// 键帽描边的**笔宽**（逻辑像素）：恒为 **1 整像素**。
///
/// 单一来源——描边矩形的内缩量、笔宽、填色矩形的内缩量三处都取它，改一处就够。
///
/// # 为什么必须是整数
///
/// 曾经的 1.5px 是"一整行 + 半行"的组合，而那半行往哪边摊取决于矩形的**小数尾巴**：
/// 同一个键帽上右边被摊成 2 行、左边只有 1 行（用户报"右侧和上方看起来更厚"），
/// 换一颗键（小数尾巴不同）变厚的边又换成另一条 —— 看着像随机。
/// 配 [`Geom::rect_of`] 的整数吸附后，1px 的墨迹正好盖住最外一圈像素，四边等厚。
pub const BORDER_W: f32 = 1.0;

/// 实际的淡出时长：**跟随 `hold`，上限 [`FADE_SEC`]**。
///
/// # 为什么不恒定
///
/// 面板给「高亮保持」留了 `0`（= 松开即灭）。若淡出恒为 0.25s，用户把滑块拖到底、
/// 选到 0.1 甚至 0，屏幕上仍会看到一段余光——「选了最小值还是灭不掉」，2026-09-18
/// 用户实测报的就是这条。改成本函数后：
///
/// | `hold` | 淡出 | 松开后可见总时长 |
/// |---|---|---|
/// | 0    | 0    | **0（硬切）** |
/// | 0.1  | 0.1  | 0.2s（旧行为是 0.35s） |
/// | ≥0.25| 0.25 | 与旧行为完全一致 |
///
/// 上界保留是为了不改变大保持时长的手感：`hold = 2.0` 时仍是 0.25s 淡出，
/// 否则「保持 2 秒」会连淡出一起被拉成 2 秒，尾巴长得像卡住。
pub fn fade_sec(hold: f64) -> f64 {
    hold.clamp(0.0, FADE_SEC as f64)
}

/// 键位在屏幕上的落点（物理像素）。
#[derive(Clone, Debug, PartialEq)]
pub struct KeyBox {
    pub rect: Rect,
    /// 当前高亮强度 0..=1（0 = 未按）。
    pub glow: f32,
    /// 要显示的图例。
    pub label: String,
    /// 布局里的键定义：键码、样式名、单键圆角都从它来。
    pub key: KeyDef,
}

impl KeyBox {
    /// 物理键码。
    pub fn code(&self) -> KeyCode {
        self.key.code
    }
}

/// 虚拟键盘模块。
pub struct VirtualKeyboardModule {
    /// 虚拟桌面矩形（物理像素）。由 Runtime 每帧注入。
    screen: ScreenRect,
    /// 挂钟起点。高亮计时基于真实时间，不能用帧时钟（停帧期间它不走）。
    clock_start: std::time::Instant,
    /// 最近一次按下的时刻（挂钟秒）。松开时**刷新为松开的时刻**，
    /// 于是 `hold` 从松开算起——按住 10 秒的 Ctrl 松开后也该亮 1 秒。
    press_at: HashMap<KeyCode, f64>,
    /// 当前物理按住的键。
    held: HashSet<KeyCode>,
    /// Shift 是否处于按下态（决定图例取 `shift` 还是 `base`）。
    shift: bool,
    /// 最近一次 tick 看到的「高亮保持时长」。`next_change_ms` 拿不到 config，
    /// 只能由 tick 顺手存下来（改动来自面板时下一帧就生效）。
    hold: f64,
    /// 当前方案是否含滚轮键帽（见 [`Self::wants_wheel`]）。
    wants_wheel: bool,
    /// 当前方案是否含鼠标按键键帽（见 [`Self::wants_mouse`]）。
    wants_mouse: bool,
}

impl VirtualKeyboardModule {
    pub fn new(_config: &Config) -> Self {
        Self {
            screen: ScreenRect::FALLBACK,
            clock_start: std::time::Instant::now(),
            press_at: HashMap::new(),
            held: HashSet::new(),
            shift: false,
            // 与 `default_vk_hold()` 一致：模块创建到第一次 tick 之间若用别的值，
            // 首帧判定会与用户设定不一致。
            hold: 0.1,
            wants_wheel: false,
            wants_mouse: false,
        }
    }

    /// 当前方案里有没有滚轮键帽（`WheelUp`/`WheelDown`）。
    ///
    /// 由 `tick` 每帧顺手刷新（tick 本来就要构建 `VkConfig`），供
    /// `Runtime::sync_input_gate` 决定是否武装滚轮门 —— 方案里没有滚轮键帽就不
    /// 转发滚轮，省掉"每滚一格唤醒一帧"的开销。
    pub fn wants_wheel(&self) -> bool {
        self.wants_wheel
    }

    /// 当前方案里有没有鼠标按键键帽（左/右/中/侧键）。
    ///
    /// 同样由 `tick` 每帧刷新，供 `Runtime::sync_input_gate` 决定要不要为虚拟键盘
    /// 打开**鼠标按下/抬起**的门。
    ///
    /// ★★ 这条门**不受「鼠标穿透」影响**（2026-09-26 修）：穿透只决定"点键帽要不要
    /// 注入真实按键"，而"点亮键帽"是另一回事，它只依赖"有没有收到这条鼠标事件"。
    /// 焊在一起的下场是：勾着穿透的方案（WASD 游戏板出厂即勾）鼠标键帽永远不亮。
    pub fn wants_mouse(&self) -> bool {
        self.wants_mouse
    }

    /// 注入当前虚拟桌面矩形。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
    }

    /// 当前挂钟时间（秒）。
    pub fn now_s(&self) -> f64 {
        self.clock_start.elapsed().as_secs_f64()
    }

    /// 直接记一次按下 / 抬起（**不经过输入事件**）。
    ///
    /// 给「点键帽」那条路用：那里已经用 `hit::claim` 判过命中、并且**主动吃掉了**
    /// 这条鼠标事件（不给拖尾留一笔），事件就不会再经 `tick` 进来 —— 不在这里记一笔，
    /// 点的那颗键帽自己反而不亮（2026-09-26 修：鼠标键帽正是如此，点了完全没反应）。
    ///
    /// `down = false` 只是松开：高亮从**这一刻**起走 `hold` + 淡出，与 `KeyUp` 同款。
    pub fn note_press(&mut self, code: KeyCode, down: bool) {
        let now = self.now_s();
        if down {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
        self.press_at.insert(code, now);
    }

    /// 是否还有任何键在亮（帧门用）。
    pub fn has_glow(&self) -> bool {
        !self.press_at.is_empty()
    }

    /// 下次画面会变的时刻距现在多少毫秒；`None` = 已经静止。
    ///
    /// ★ 报的是「下次**画面会变**」而不是「到彻底消失」—— 这两个在淡出区间里不是
    ///   一回事：淡出的每一帧 `glow` 都在变（`glow_of` 那条 1→0 的斜坡）。报"到消失"
    ///   会让平台**一觉睡过整段淡出**，斜坡一帧都不落笔，表现是**到点突然熄灭**而不是
    ///   淡出（`hold` 与 `FADE_SEC` 这两个参数于是都看不出效果）。与击键回显
    ///   `key_display::KeyController::next_change_ms` 同款：淡出期间报 0（= 连帧），
    ///   平台按 `FrameDemand::Now` 连续出帧，直到 `sweep` 把过期项清掉、自然回 `Idle`。
    ///   代价有上限：斜坡最长 `FADE_SEC`（0.25s）。
    pub fn next_change_ms(&self) -> Option<u32> {
        if self.press_at.is_empty() {
            return None;
        }
        if !self.held.is_empty() {
            // 修饰键按住期间画面不变，但一旦松开就要重画——没有定时点可报，
            // 只能让调用方按低频轮询（返回值 0 = 现在就再出一帧会把 CPU 钉满，
            // 故给一个保守的 100ms）。
            return Some(100);
        }
        let now = self.now_s();
        let fade = fade_sec(self.hold);
        let mut soonest = f32::MAX;
        for at in self.press_at.values() {
            let left = (at + self.hold + fade) - now;
            if left <= 0.0 {
                // 已过期：等下一帧 `sweep` 收走它，这里不必再报时间。
                continue;
            }
            // 过了 `hold`、正走在淡出斜坡上 → 连帧（理由见函数头）。
            // `fade == 0`（hold = 0，松开即灭）时没有斜坡可看，不进这一支。
            if fade > 0.0 && now > at + self.hold {
                return Some(0);
            }
            soonest = soonest.min(left as f32);
        }
        if soonest == f32::MAX {
            None
        } else {
            Some((soonest * 1000.0).ceil().max(1.0) as u32)
        }
    }

    /// 处理一条输入事件。
    fn on_event(&mut self, ev: &InputEvent) {
        let now = self.now_s();
        match *ev {
            InputEvent::KeyDown { key, mods, .. } => {
                // ★ 归一到布局表里的形态再记：捕获层给的 `Char` 已按 Shift 修正
                // （Shift+1 → `'!'`），而表里存的是 `'1'`。不归一就永远匹配不上。
                let code = normalize(key);
                self.shift = mods.shift || self.shift;
                self.held.insert(code);
                self.press_at.insert(code, now);
            }
            InputEvent::KeyUp { key, mods, .. } => {
                let code = normalize(key);
                self.shift = mods.shift;
                self.held.remove(&code);
                // 从**松开**重新计时。
                self.press_at.insert(code, now);
            }
            // 鼠标按键：左/右/中/侧键（方案里写对应键码的键帽都会亮）。滚轮在
            // 下面单独处理（脉冲事件）。
            InputEvent::MouseDown { button, .. } => {
                let Some(code) = mouse_code(button) else {
                    return;
                };
                self.held.insert(code);
                self.press_at.insert(code, now);
            }
            InputEvent::MouseUp { button, .. } => {
                let Some(code) = mouse_code(button) else {
                    return;
                };
                self.held.remove(&code);
                self.press_at.insert(code, now);
            }
            // 滚轮是**脉冲**事件（没有对应的"抬起"）：只记一次按下时刻，走既有的
            // 保持/淡出时间线（sweep 会收走）。横向倾斜不高亮 —— 键帽里没有它。
            InputEvent::MouseWheel {
                delta, horizontal, ..
            } if !horizontal => {
                if let Some(code) = wheel_code(delta) {
                    self.press_at.insert(code, now);
                }
            }
            _ => {}
        }
        if self.held.contains(&KeyCode::LShift) || self.held.contains(&KeyCode::RShift) {
            self.shift = true;
        }
    }

    /// 一个键当前的高亮强度（0..=1）。
    fn glow_of(&self, code: KeyCode, now: f64, hold: f64) -> f32 {
        let Some(at) = self.press_at.get(&code) else {
            return 0.0;
        };
        if self.held.contains(&code) {
            return 1.0;
        }
        let age = now - at;
        if age <= hold {
            1.0
        } else {
            // ★ 淡出时长跟随 `hold`（上限 `FADE_SEC`）。`hold = 0` 时 `fade = 0`，
            //   这里必须显式短路——否则下面会做 0/0，得到 NaN，`contains` 判假后返回 0.0
            //   虽然"结果碰巧对"，但把 NaN 当成正常路径太脆（NaN 比较恒假，一旦以后
            //   有人把这段改成 `if t > 0.0`，行为就整个翻过来）。
            let fade = fade_sec(hold);
            if fade <= 0.0 {
                return 0.0;
            }
            let t = 1.0 - ((age - hold) as f32 / fade as f32);
            if !(0.0..=1.0).contains(&t) {
                0.0
            } else {
                t
            }
        }
    }

    /// 清掉已经淡完的键。每帧调一次，否则 `press_at` 会无限增长。
    fn sweep(&mut self, now: f64, hold: f64) {
        let dead: Vec<KeyCode> = self
            .press_at
            .iter()
            .filter(|(k, at)| !self.held.contains(k) && now - *at > hold + fade_sec(hold))
            .map(|(k, _)| *k)
            .collect();
        for k in dead {
            self.press_at.remove(&k);
        }
    }
}

impl Module for VirtualKeyboardModule {
    fn enabled(&self, config: &Config) -> bool {
        config.virtual_keyboard.enabled
    }

    fn tick(&mut self, _tick: &Tick, config: &Config, input: &Option<InputEvent>) {
        // 关着的期间也要把事件吞掉、状态清干净：否则重新打开时会看到一堆
        // 几分钟前按过的键还亮着。
        if !config.virtual_keyboard.enabled {
            if !self.press_at.is_empty() || !self.held.is_empty() {
                self.press_at.clear();
                self.held.clear();
                self.shift = false;
            }
            return;
        }
        // 高亮保持是方案控件（`defaults.hold`）的目标 —— 走 VkConfig 才认方案里写的值。
        let vk_cfg = vk_config::VkConfig::load(&config.virtual_keyboard);
        self.hold = vk_cfg.hold();
        // 顺手刷新两道门的标记（tick 本来就构建了 VkConfig，不额外解析）。
        self.wants_wheel = vk_cfg.wants_wheel();
        self.wants_mouse = vk_cfg.wants_mouse();
        let now = self.now_s();
        if let Some(ev) = input {
            self.on_event(ev);
        }
        self.sweep(now, self.hold);
    }

    fn draw(&self, list: &mut DrawList, config: &Config) {
        let s: &VirtualKeyboardSettings = &config.virtual_keyboard;
        if !s.enabled {
            return;
        }
        // build() 需要 &mut self，但 draw 是 &self——这里用一次性构建。
        // VkConfig 的构建成本很低（解析一次 JSON + 合并几个字段），可以每帧重建。
        let vk_cfg = vk_config::VkConfig::load(s);
        let boxes = self.layout_boxes_with(s, &vk_cfg);
        if boxes.is_empty() {
            return;
        }
        let dpi = crate::platform::display::dpi_scale();

        // 总不透明度：容器级乘子，与各颜色自带的 alpha **相乘**
        // （`DrawCmd::PushAlpha` 由渲染层的 alpha 栈实现，见 `runtime::render`）。
        // 值来自方案控件（`defaults.opacity`），不是 config 字段本身。
        list.push_alpha(vk_cfg.opacity());
        for b in &boxes {
            let pressed = b.glow > 0.0;
            let style = vk_cfg.resolve_style(&b.key, pressed);
            // 圆角：**0 就是直角**，不要去碰它。这里原本是 `.max(1.0)`，把"直角"这一档
            // 直接吃掉了 —— 面板拖到 0 也还是 1px 圆角，用户看到的"设置无效"。
            // 也不再按"键高的 14%"自动算：一个参数只回答一个问题，不留隐式分支。
            let radius = style.radius.max(0.0);

            // ★ 侧壁（`styles.<n>.side` + `depth`，2026-09-28）：键帽立体感的通用画法。
            //   先画"下移 depth 的侧壁键帽"，再画本体 —— 侧壁从底边露出一圈即厚度。
            //   按下时本体**整体下移 depth**：底边与侧壁底对齐，像被按进底座
            //   （即"按下吃掉硬阴影"）；松开弹回。无侧壁（alpha=0 或
            //   depth=0）两条路径与旧版逐字节一致，出厂方案零变化。
            let depth = if style.side.a() > 0 { style.depth } else { 0.0 };
            if depth > 0.0 {
                list.fill_round_rect(
                    Rect::new(b.rect.x, b.rect.y + depth, b.rect.w, b.rect.h),
                    radius,
                    style.side,
                );
            }
            let shift = if pressed { depth } else { 0.0 };

            // 描边：**真描边（空心）** + 填色。
            //
            // 原实现是"先用边框色填满整颗键帽，再压一颗内缩 1.5px 的填色"——
            // 不透明时看不出问题，一旦键帽色/总不透明度带上 alpha，"内衬"那层会把
            // 边框色混进来，键帽主体 = 混色(border, fill) 而不是 fill。
            //
            // ★★ 笔宽 **1 整像素**（2026-09-26，原 1.5）：描边矩形内缩半个笔宽后，
            //   墨迹正好盖住键帽最外一圈像素 —— 四边厚度**完全一样**。
            //   1.5px 是"一整行 + 半行"的组合：矩形哪怕只差 0.1px，那半行往哪边摊
            //   都不一样厚，于是"右边厚、左边薄"（详见 `Geom::rect_of` 的吸附注释）。
            //   （详见 [`BORDER_W`] 与 [`Geom::rect_of`] 的注释）
            let (stroke_rect, stroke_w, mut inner) = cap_rects(b.rect);
            let stroke_rect = Rect::new(
                stroke_rect.x,
                stroke_rect.y + shift,
                stroke_rect.w,
                stroke_rect.h,
            );
            inner = Rect::new(inner.x, inner.y + shift, inner.w, inner.h);
            list.stroke_round_rect(stroke_rect, radius, stroke_w, style.border);
            list.fill_round_rect(inner, radius, style.fill);

            if !b.label.is_empty() {
                // ★ 居中与"放不下就缩"都交给渲染层（`DrawCmd::TextInBox`）。
                //
                // 这里曾经自己凑：`x = 键帽左边 + (键宽 - estimate_width)/2`、
                // `y = 键帽上边 + (键高 - 字号*1.25)/2`。两个式子都建立在"模块层能算出
                // 文字尺寸"这个不成立的假设上 —— 于是估宽偏大 → 整体左偏（用户报
                // "Ctrl 靠左"），凑的盒高 ≠ 真实行高 → 垂直也不居中。
                // 渲染层手上有 `measure_text` 与 `text_line_metrics`，两轴都按实测来。
                let font = vk_cfg.font_size() * dpi;
                list.text_in_box(inner, &b.label, font, style.text, None);
            }
        }
        list.pop_alpha();
    }
}

impl VirtualKeyboardModule {
    /// 发布点击命中区给鼠标钩子线程（见 `platform/input/hit`）。
    ///
    /// 判据只有两条：**键盘开着** 且 **没勾「鼠标穿透」**。其余情况发布非活跃标记，
    /// 钩子那边一律放行（点击照常落到下面的窗口）。
    ///
    /// ★ **不再看控制面板开没开**（2026-09-18 二次修订）。面板占着时把命中区整体关掉，
    /// 是当初为"HEX 框点不进去"打的补丁；但那个补丁是**全局**的——只要面板还开着，键盘
    /// 就永远点不动，而且**与「鼠标穿透」勾不勾无关**（`panel_open` 在判据最前面短路）。
    /// 用户实测报的正是"无论是否勾选，永远具有鼠标穿透效果"。
    ///
    /// 正解是**按落点**让路：点在面板 / 取色器弹窗 / 贴图窗这些**我们自己的窗口**上时，
    /// 钩子本来就不该吞（见 `platform/input/hit::claim` 里的 `over_our_window`）。
    /// 面板与键帽重叠 → 这次点击归面板；不重叠 → 键帽照常可点。两条都要成立，
    /// 而只有"看落点"能同时满足。
    ///
    /// 每帧调用：`hit::publish` 内部对"内容没变"做了短路，键盘静止时只花一次比较。
    pub fn publish_hit_region(&self, s: &VirtualKeyboardSettings) {
        if !s.enabled || s.click_through {
            // `debug` 而非 `info`：这是**每帧**都会走到的分支（穿通常态 / 功能关闭），
            // 用 `info` 会把日志冲成一片（实测一次会话 9000+ 行，真信号全被淹了）。
            log::debug!(
                "publish_hit_region: active=false (enabled={}, click_through={})",
                s.enabled,
                s.click_through
            );
            crate::platform::input::hit::publish(Vec::new(), false);
            return;
        }
        let vk_cfg = vk_config::VkConfig::load(s);
        let rects = self.layout_rects_with(s, &vk_cfg);
        log::debug!("publish_hit_region: active=true, {} keycaps", rects.len());
        crate::platform::input::hit::publish(rects, true);
    }

    /// 只算「键码 + 矩形」，不算图例、不查高亮。
    ///
    /// 命中区发布（[`publish_hit_region`](Self::publish_hit_region)）**每帧**都要一份
    /// 键位落点，而它只需要这两样东西。走 [`layout_boxes_with`](Self::layout_boxes_with)
    /// 会顺带每颗键克隆一次 `KeyDef`（两个 `String`）、每帧查一次 `GetKeyState`——
    /// 165 颗键 × 每次出帧，纯属白费。所以把几何部分单独摘出来。
    pub fn layout_rects_with(
        &self,
        s: &VirtualKeyboardSettings,
        vk_cfg: &vk_config::VkConfig,
    ) -> Vec<(KeyCode, Rect)> {
        let g = self.geom(s, vk_cfg);
        let lay = vk_cfg.layout();
        let mut out = Vec::with_capacity(165);
        for (r, row) in lay.rows.iter().enumerate() {
            let mut x_u = 0.0f32;
            for k in &row.keys {
                x_u += k.pad;
                out.push((k.code, g.rect_of(x_u, r as f32, k)));
                x_u += k.w;
            }
        }
        out
    }

    /// 键位几何的公共骨架：单位尺寸、空隙、整个键盘的左上角。
    ///
    /// `layout_boxes`（要画）与 `layout_rects`（只要命中区）都从这里出发 —— 两条路
    /// 各算一遍几何迟早会漂移，而漂移的后果是「看得见的键帽」与「点得中的键帽」错位。
    fn geom(&self, s: &VirtualKeyboardSettings, vk_cfg: &vk_config::VkConfig) -> Geom {
        let lay = vk_cfg.layout();
        let dpi = crate::platform::display::dpi_scale();
        // ★ 键帽边长与空隙**只从 VkConfig 取**：它们是方案控件（`defaults.keySize` /
        // `defaults.gap` + 缩放）的目标，直接读 config 字段会绕开方案与面板控件。
        let unit = vk_cfg.unit() * dpi;
        let gap = vk_cfg.gap() * dpi;
        let step = unit + gap;

        // 布局总宽（u）：取最宽的一行。
        let mut total_u = 0.0f32;
        for row in &lay.rows {
            let mut x = 0.0f32;
            for k in &row.keys {
                x += k.pad + k.w;
            }
            total_u = total_u.max(x);
        }
        let rows_n = lay.rows.len() as f32;
        let total_w = total_u * step - gap;
        let total_h = rows_n * step - gap;

        let scr = self.screen;
        let (is_left, is_top) = split_placement(&s.placement);
        let x0 = if is_left {
            scr.x as f32 + scr.w as f32 * pct(s.left_offset)
        } else {
            scr.x as f32 + scr.w as f32 - total_w - scr.w as f32 * pct(s.right_offset)
        };
        let y0 = if is_top {
            scr.y as f32 + scr.h as f32 * pct(s.top_offset)
        } else {
            scr.y as f32 + scr.h as f32 - total_h - scr.h as f32 * pct(s.bottom_offset)
        };
        Geom {
            unit,
            gap,
            step,
            x0,
            y0,
        }
    }

    /// 按当前配置算出所有键位的屏幕矩形与高亮强度。
    ///
    /// 纯计算、不改状态，方便单测直接断言几何。
    pub fn layout_boxes(&self, s: &VirtualKeyboardSettings) -> Vec<KeyBox> {
        let vk_cfg = vk_config::VkConfig::load(s);
        self.layout_boxes_with(s, &vk_cfg)
    }

    /// 使用已有的 VkConfig 计算键位布局（draw 时复用，避免重复加载 JSON）。
    pub fn layout_boxes_with(
        &self,
        s: &VirtualKeyboardSettings,
        vk_cfg: &vk_config::VkConfig,
    ) -> Vec<KeyBox> {
        // 几何来自**当前方案**（`assets/VirtualKeyboards/<方案>/keyboard.json`，
        // 见 `vk_config`），不再是编译期的静态表。
        let g = self.geom(s, vk_cfg);
        let lay = vk_cfg.layout();

        let now = self.now_s();
        let hold = vk_cfg.hold();
        let caps = crate::platform::win_util::caps_lock_on();
        let shift = self.shift;

        let mut out = Vec::with_capacity(120);
        for (r, row) in lay.rows.iter().enumerate() {
            let mut x_u = 0.0f32;
            for k in &row.keys {
                x_u += k.pad;
                let rect = g.rect_of(x_u, r as f32, k);
                // 图例的唯一真相来源是几何表（`k` 自带 base/shift 两层 + caps 标志），
                // 上档文字随 Shift 恒生效（不再有"随 Shift 切换"开关）。
                let label = vk_cfg.resolve_label(k, shift, caps);
                out.push(KeyBox {
                    rect,
                    glow: self.glow_of(k.code, now, hold),
                    label,
                    key: k.clone(),
                });
                x_u += k.w;
            }
        }
        out
    }
}

/// 百分比 → 比例（`0.5` → `0.005`）。
///
/// ★ 参数是 `f32`：偏移出厂值带小数（水平 0.5%），整数存不下。
fn pct(v: f32) -> f32 {
    v.clamp(0.0, 100.0) / 100.0
}

/// 一份键位几何的公共骨架（见 [`VirtualKeyboardModule::geom`]）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geom {
    /// 标准键帽边长（物理像素）。
    unit: f32,
    /// 键间空隙（物理像素）。
    gap: f32,
    /// 相邻键的步长（= `unit + gap`）。
    step: f32,
    /// 整个键盘左上角（虚拟桌面物理像素）。
    x0: f32,
    y0: f32,
}

impl Geom {
    /// 某颗键的落点。`x_u` = 该键左边缘在行内的 u 坐标，`r` = 行号。
    ///
    /// 宽跨 `n` 个 u 的键（Tab 1.5 / Space 6.25…）要补上中间那 `n-1` 道空隙，
    /// 否则它们会比"n 个标准键并排"窄 —— 这是本模块最容易被忽略的一处。
    fn rect_of(&self, x_u: f32, r: f32, k: &layout::KeyDef) -> Rect {
        let x = self.x0 + x_u * self.step;
        let y = self.y0 + r * self.step;
        let w = k.w * self.unit + (k.w - 1.0).max(0.0) * self.gap;
        let h = k.h * self.unit + (k.h - 1.0).max(0.0) * self.gap;
        // ★★ 吸附到**整数物理像素**（2026-09-26 用户报告"键帽描边一边厚一边薄，
        //   不同键帽变厚的边还不一样"）。
        //
        //   根因不是描边代码写错，而是矩形落在了半像素上：`x0` 由"屏幕宽的百分比偏移"
        //   算出来（出厂水平偏移 0.5% 时就带小数），于是每颗键的 `x` 都带着同一个小数尾巴。
        //   1px 的描边条带压在 `x.6` 上会被抗锯齿摊成三行（0.4 / 1.0 / 0.1），
        //   对面那条压在 `x+w-1` 上又是另一种分摊 —— 于是"右/上厚、左/下薄"，
        //   而每颗键的小数尾巴不同，看着就像"变厚的边随机"。
        //
        //   吸附用「右边界 = round(x+w) - round(x)」而不是各自取整，相邻键因此共享同一条
        //   边界像素：`gap = 0` 时严丝合缝、不会漏出一道背景色细缝。
        let sx = x.round();
        let sy = y.round();
        Rect::new(sx, sy, (x + w).round() - sx, (y + h).round() - sy)
    }
}

/// 一颗键帽的**（描边矩形, 笔宽, 填色矩形）**。
///
/// 抽成纯函数只为一件件事：**能单测**。"描边四边是否等厚"这件事完全由这三个数决定
/// ——矩形落在半像素上、或笔宽不是整数，墨迹就会被抗锯齿摊成厚薄不一的一圈
/// （现象与成因见 [`BORDER_W`]）。留在 `draw` 里就没法断言。
fn cap_rects(rect: Rect) -> (Rect, f32, Rect) {
    let hw = BORDER_W * 0.5;
    (
        Rect::new(
            rect.x + hw,
            rect.y + hw,
            (rect.w - BORDER_W).max(0.5),
            (rect.h - BORDER_W).max(0.5),
        ),
        BORDER_W,
        Rect::new(
            rect.x + BORDER_W,
            rect.y + BORDER_W,
            (rect.w - BORDER_W * 2.0).max(1.0),
            (rect.h - BORDER_W * 2.0).max(1.0),
        ),
    )
}

/// 位置字符串 → (靠左, 靠上)。
fn split_placement(p: &str) -> (bool, bool) {
    let left = p.ends_with("_left");
    let top = p.starts_with("top_");
    (left, top)
}

/// `MouseButton` → 虚拟键盘关心的键码（左/右/中/侧键）。滚轮走 `MouseWheel` 单独归一。
fn mouse_code(b: crate::core::events::MouseButton) -> Option<KeyCode> {
    use crate::core::events::MouseButton as Mb;
    match b {
        Mb::Left => Some(KeyCode::MouseLeft),
        Mb::Right => Some(KeyCode::MouseRight),
        Mb::Middle => Some(KeyCode::MouseMiddle),
        Mb::X1 => Some(KeyCode::MouseX1),
        Mb::X2 => Some(KeyCode::MouseX2),
    }
}

/// 滚轮事件 → 键码（脉冲：上/下）。横向倾斜滚轮没有对应键帽，不归它管。
fn wheel_code(delta: i32) -> Option<KeyCode> {
    if delta > 0 {
        Some(KeyCode::WheelUp)
    } else {
        Some(KeyCode::WheelDown)
    }
}

/// 把 `KeyCode` 归一到布局表里用的形态。
///
/// 捕获层给的 `Char(c)` 已经按 Shift 修正过（按 Shift+1 给的是 `'!'`），而布局表
/// 里存的是**未修正**的键位（`Char('1')`）。不归一就永远匹配不上——这是本模块
/// 最容易踩的一处。
pub fn normalize(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) => {
            if c == ' ' {
                return KeyCode::Space;
            }
            if c.is_ascii_alphanumeric() {
                return KeyCode::Char(c.to_ascii_lowercase());
            }
            KeyCode::Char(unshift(c))
        }
        _ => code,
    }
}

/// 上档字符 → 下档字符。
fn unshift(c: char) -> char {
    match c {
        '~' => '`',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Modifiers;

    /// 测试用的配置：键盘开着 + **不穿透**。
    ///
    /// ★ 穿透必须显式关掉：出厂默认就是 true（104 键太宽，默认不吃鼠标），
    ///   而"命中区"只在**不穿透**时才发布 —— 这两个用例验的正是那条链路。
    fn cfg() -> Config {
        let mut c = Config::default();
        c.virtual_keyboard.enabled = true;
        c.virtual_keyboard.click_through = false;
        c
    }

    fn key(c: char, shift: bool) -> InputEvent {
        InputEvent::KeyDown {
            key: KeyCode::Char(c),
            mods: Modifiers {
                shift,
                ..Default::default()
            },
            repeated: false,
            time_ms: 0,
        }
    }

    #[test]
    fn press_marks_glow() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&t, &c, &Some(key('a', false)));
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        let a = boxes
            .iter()
            .find(|b| b.label.eq_ignore_ascii_case("A"))
            .expect("布局里应有 A 键");
        assert!(a.glow > 0.9, "刚按下的 A 应该全亮，实际 {}", a.glow);
    }

    #[test]
    fn shift_switches_legend() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&t, &c, &Some(key('1', true)));
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        // 只看主键盘那颗 `1`（第 1 行、base 为 "1"、shift 为 "!" 的那颗）：
        // 小键盘也有一颗恒为 "1" 的键，拿全集判断会误判。
        let one = boxes
            .iter()
            .find(|b| b.label == "1" || b.label == "!")
            .expect("主键盘应有 1 键");
        assert_eq!(one.label, "!", "Shift 态应显示 '!'，实际 {}", one.label);
    }

    #[test]
    fn release_starts_hold_window() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&t, &c, &Some(key('a', false)));
        let up = InputEvent::KeyUp {
            key: KeyCode::Char('a'),
            mods: Modifiers::default(),
            time_ms: 10,
        };
        m.tick(&t, &c, &Some(up));
        // 松开后仍在保持窗口内：高亮不该立刻消失。
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        // ★ 按**键码**找，不按标签找：标签随真机 CapsLock/Shift 变大小写
        // （`caps_lock_on()` 读的是物理键灯），按 "a" 找会让测试随环境翻红。
        let a = boxes
            .iter()
            .find(|b| b.key.code == KeyCode::Char('a'))
            .expect("有 A 键");
        assert!(
            a.label.eq_ignore_ascii_case("a"),
            "标签应是 a/A，实际 {}",
            a.label
        );
        assert!(a.glow > 0.9, "松开后应保持高亮，实际 {}", a.glow);
    }

    #[test]
    fn disabled_clears_state() {
        let mut c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&t, &c, &Some(key('a', false)));
        c.virtual_keyboard.enabled = false;
        m.tick(&t, &c, &None);
        assert!(!m.has_glow(), "关掉后不该还有高亮残留");
    }

    /// ★ 回归（2026-09-18）：**淡出区间里必须连帧**，否则 `hold` 与 `fade_sec` 都白设。
    ///
    /// 报"到彻底消失"的毫秒数会让平台一觉睡过整段淡出 —— `glow_of` 那条 1→0 的斜坡
    /// 一帧都不落笔，看到的是"到点突然熄灭"而不是淡出。与击键回显
    /// `KeyController::next_change_ms` 同款：淡出期间报 0（= 连帧）。
    /// （`hold` 取量程里很小的 0.05s，用例才不必真等半秒墙钟。）
    #[test]
    fn next_change_ms_keeps_frames_during_fade() {
        let mut c = cfg();
        c.virtual_keyboard.hold = 0.05;
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        // 按下再松开：`held` 清空、`press_at` 从**抬起**重新计时 → 之后走 hold + fade。
        m.tick(&t, &c, &Some(key('a', false)));
        m.tick(
            &t,
            &c,
            &Some(InputEvent::KeyUp {
                key: KeyCode::Char('a'),
                mods: Modifiers::default(),
                time_ms: 10,
            }),
        );

        // hold 区间内：报"到淡出开始"的剩余时间 —— 既不该是连帧(0)，也不该是 None。
        let in_hold = m.next_change_ms().expect("保持窗口内应有下一次变化");
        assert!(
            in_hold > 0,
            "hold 区间内应报到淡出开始的时刻、而不是连帧；实得 {in_hold}"
        );

        // 睡过 hold(0.05s) → 已进淡出斜坡。
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(
            m.next_change_ms(),
            Some(0),
            "已进淡出区间 → 必须报 0（连帧），否则斜坡一帧都不会被画出来"
        );
    }

    #[test]
    fn normalize_maps_shifted_chars_back() {
        assert_eq!(normalize(KeyCode::Char('!')), KeyCode::Char('1'));
        assert_eq!(normalize(KeyCode::Char('A')), KeyCode::Char('a'));
        assert_eq!(normalize(KeyCode::Char(' ')), KeyCode::Space);
        assert_eq!(normalize(KeyCode::Char('?')), KeyCode::Char('/'));
        assert_eq!(normalize(KeyCode::F(5)), KeyCode::F(5));
    }

    /// ★ 游戏板（2026-09-26）：鼠标左/右键按下要能点亮对应键帽。
    ///
    /// 通路是 `InputEvent::MouseDown/MouseUp` → `mouse_code` → `KeyCode::MouseLeft/Right`
    /// —— 键码是新增变体，只作显示用（`to_vk` 不认，快捷键也注册不了）。
    #[test]
    fn mouse_down_lights_the_game_board() {
        let mut c = cfg();
        c.virtual_keyboard.layout = "wasd_gaming".to_string();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(
            &t,
            &c,
            &Some(InputEvent::MouseDown {
                button: crate::core::events::MouseButton::Left,
                x: 0,
                y: 0,
                time_ms: 0,
            }),
        );
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        let lmb = boxes
            .iter()
            .find(|b| b.key.code == KeyCode::MouseLeft)
            .expect("游戏板应有「左键」键帽");
        assert!(lmb.glow > 0.9, "左键按下应全亮，实际 {}", lmb.glow);
        let rmb = boxes
            .iter()
            .find(|b| b.key.code == KeyCode::MouseRight)
            .expect("游戏板应有「右键」键帽");
        assert_eq!(rmb.glow, 0.0, "右键不该被左键带着亮");

        // 松开 → 从松开时刻起保持（与键盘键同一条时间线）。
        m.tick(
            &t,
            &c,
            &Some(InputEvent::MouseUp {
                button: crate::core::events::MouseButton::Left,
                x: 0,
                y: 0,
                time_ms: 10,
            }),
        );
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        let lmb = boxes
            .iter()
            .find(|b| b.key.code == KeyCode::MouseLeft)
            .expect("应有「左键」键帽");
        assert!(lmb.glow > 0.9, "松开后应仍在保持窗口内，实际 {}", lmb.glow);
    }

    /// ★★ 「点键帽」那条路也要点亮（2026-09-26 修）。
    ///
    /// 点在键帽上时 `Runtime::vk_pointer_begin` 会**主动吃掉**这条鼠标事件（不给拖尾
    /// 留一笔），事件因此进不了 `tick`。鼠标键帽更是连注入都做不到（没有可注入的 VK），
    /// 旧实现于是"注入失败 + 吞掉事件 + 键帽不亮"三头落空 —— 用户看到的就是"点了没反应"。
    /// [`VirtualKeyboardModule::note_press`] 是这条路的补记入口，这里钉住它。
    #[test]
    fn note_press_lights_a_keycap_without_an_input_event() {
        let mut c = cfg();
        c.virtual_keyboard.layout = "wasd_gaming".to_string();
        let mut m = VirtualKeyboardModule::new(&c);
        assert_eq!(m.glow_of(KeyCode::MouseLeft, m.now_s(), 0.1), 0.0);

        m.note_press(KeyCode::MouseLeft, true);
        assert!(
            m.glow_of(KeyCode::MouseLeft, m.now_s(), 0.1) > 0.9,
            "记一次按下应立刻点亮"
        );
        assert!(m.held.contains(&KeyCode::MouseLeft), "按下态要记进 held");

        // 抬起：移出 held，高亮交给保持/淡出时间线（与 KeyUp 同款）。
        m.note_press(KeyCode::MouseLeft, false);
        assert!(!m.held.contains(&KeyCode::MouseLeft));
        assert!(
            m.glow_of(KeyCode::MouseLeft, m.now_s(), 0.1) > 0.9,
            "松开后仍应在保持窗口内"
        );
        assert!(
            m.glow_of(KeyCode::MouseRight, m.now_s(), 0.1) == 0.0,
            "不该波及另一颗键"
        );
    }

    /// ★ 滚轮是脉冲事件（没有"抬起"）：上/下滚各记一次按下时刻，走保持/淡出
    ///   时间线、由 sweep 收走；横向倾斜不记。中/侧键与普通键同一条按下通路。
    ///
    /// 直接断言 `press_at`（键帽级点亮已在 `mouse_down_lights_the_game_board`
    /// 验过同一条通路，这里不重复摆几何）。
    #[test]
    fn wheel_and_extra_mouse_buttons_reach_the_timeline() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(
            &t,
            &c,
            &Some(InputEvent::MouseWheel {
                delta: 120,
                horizontal: false,
                x: 0,
                y: 0,
                time_ms: 0,
            }),
        );
        assert!(m.press_at.contains_key(&KeyCode::WheelUp), "上滚要记时刻");
        assert!(
            !m.press_at.contains_key(&KeyCode::WheelDown),
            "下滚不该被带上"
        );

        m.tick(
            &t,
            &c,
            &Some(InputEvent::MouseWheel {
                delta: -120,
                horizontal: false,
                x: 0,
                y: 0,
                time_ms: 0,
            }),
        );
        assert!(m.press_at.contains_key(&KeyCode::WheelDown), "下滚要记时刻");

        // 横向倾斜：不记（键帽里没有它）。
        m.press_at.clear();
        m.tick(
            &t,
            &c,
            &Some(InputEvent::MouseWheel {
                delta: 120,
                horizontal: true,
                x: 0,
                y: 0,
                time_ms: 0,
            }),
        );
        assert!(m.press_at.is_empty(), "横向倾斜不该点亮任何键帽");

        // 中键/侧键按下 → 记"按住"；抬起 → 清。
        for (btn, code) in [
            (
                crate::core::events::MouseButton::Middle,
                KeyCode::MouseMiddle,
            ),
            (crate::core::events::MouseButton::X1, KeyCode::MouseX1),
            (crate::core::events::MouseButton::X2, KeyCode::MouseX2),
        ] {
            m.press_at.clear();
            m.held.clear();
            m.tick(
                &t,
                &c,
                &Some(InputEvent::MouseDown {
                    button: btn,
                    x: 0,
                    y: 0,
                    time_ms: 0,
                }),
            );
            assert!(
                m.held.contains(&code) && m.press_at.contains_key(&code),
                "{code:?} 按下要进时间线"
            );
            m.tick(
                &t,
                &c,
                &Some(InputEvent::MouseUp {
                    button: btn,
                    x: 0,
                    y: 0,
                    time_ms: 0,
                }),
            );
            assert!(!m.held.contains(&code), "{code:?} 抬起要清按住态");
        }

        // 滚轮脉冲没有"抬起"，只能靠 sweep 收走。
        m.press_at.clear();
        m.press_at.insert(KeyCode::WheelUp, m.now_s() - 10.0);
        m.sweep(m.now_s(), 0.1);
        assert!(
            !m.press_at.contains_key(&KeyCode::WheelUp),
            "过期脉冲要被 sweep 收走"
        );
    }

    /// 新鼠标键码族在"键名 → 键码 → 键名"上必须闭环（方案 JSON 的 keys 表靠这个）。
    #[test]
    fn mouse_code_names_round_trip() {
        for code in [
            KeyCode::MouseLeft,
            KeyCode::MouseRight,
            KeyCode::MouseMiddle,
            KeyCode::MouseX1,
            KeyCode::MouseX2,
            KeyCode::WheelUp,
            KeyCode::WheelDown,
        ] {
            assert_eq!(
                crate::modules::virtual_keyboard::layout::parse_code(
                    crate::modules::virtual_keyboard::layout::code_to_name(code)
                ),
                Some(code),
                "{code:?} 往返失败"
            );
        }
    }

    #[test]
    fn geometry_fits_screen() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        m.set_screen(ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        });
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        assert!(!boxes.is_empty());
        for b in &boxes {
            assert!(
                b.rect.w > 0.0 && b.rect.h > 0.0,
                "键帽尺寸应为正：{:?}",
                b.rect
            );
            assert!(
                b.rect.x >= 0.0 && b.rect.y >= 0.0,
                "不该画出屏幕左上角之外：{:?}",
                b.rect
            );
        }
        let max_x = boxes
            .iter()
            .map(|b| b.rect.x + b.rect.w)
            .fold(0.0f32, f32::max);
        let max_y = boxes
            .iter()
            .map(|b| b.rect.y + b.rect.h)
            .fold(0.0f32, f32::max);
        assert!(max_x <= 1920.0, "整块键盘该在屏内，实际右边缘 {max_x}");
        assert!(max_y <= 1080.0, "整块键盘该在屏内，实际下边缘 {max_y}");
    }

    #[test]
    fn layout_has_104_keys() {
        let c = cfg();
        let m = VirtualKeyboardModule::new(&c);
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        assert_eq!(boxes.len(), 104, "ANSI 104 布局应恰好 104 个键位");
    }

    #[test]
    fn draw_emits_commands() {
        let c = cfg();
        let mut m = VirtualKeyboardModule::new(&c);
        let t = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&t, &c, &Some(key('a', false)));
        let mut list = DrawList::new();
        m.draw(&mut list, &c);
        assert!(list.cmds().len() > 100, "104 个键帽至少该有上百条指令");
    }

    /// ★ 命中区（`layout_rects_with`）与画出来的键帽（`layout_boxes_with`）必须逐颗一致。
    ///
    /// 拆成两条路是为了省掉每帧对 165 颗键的克隆与 `GetKeyState`，代价是"两份几何"的
    /// 漂移风险。一旦漂移，症状是**看得见的键帽与点得中的键帽错位**——差几个像素肉眼
    /// 完全看不出来，但鼠标落在键帽边缘就会漏一次输入。所以把它钉成断言。
    #[test]
    fn hit_rects_match_drawn_boxes() {
        for id in ["ansi_104", "ansi_60"] {
            let mut c = cfg();
            c.virtual_keyboard.layout = id.to_string();
            let m = VirtualKeyboardModule::new(&c);
            let s = &c.virtual_keyboard;
            let vk_cfg = vk_config::VkConfig::load(s);

            let boxes = m.layout_boxes_with(s, &vk_cfg);
            let rects = m.layout_rects_with(s, &vk_cfg);
            assert_eq!(boxes.len(), rects.len(), "{id}：两条路的键数应一致");
            for (b, (code, rect)) in boxes.iter().zip(rects.iter()) {
                assert_eq!(b.code(), *code, "{id}：键序应一致");
                assert_eq!(b.rect, *rect, "{id}：{code:?} 的矩形应逐位一致");
            }
        }
    }

    /// ★ 每一颗出厂键帽都得能注入。
    ///
    /// 方案 JSON 是**用户可写的**：写了一个我们没做映射的键码，那枚键帽就会"点了没
    /// 反应"，而面板上不会有任何提示。出厂两套必须 100% 命中——这条断言就是兜住
    /// "布局加了新键、`to_vk` 忘了跟"的那一类改动。
    #[test]
    fn every_builtin_key_is_injectable() {
        for id in ["ansi_104", "ansi_60"] {
            let mut c = cfg();
            c.virtual_keyboard.layout = id.to_string();
            let m = VirtualKeyboardModule::new(&c);
            let s = &c.virtual_keyboard;
            let vk_cfg = vk_config::VkConfig::load(s);
            for (code, _) in m.layout_rects_with(s, &vk_cfg) {
                assert!(
                    crate::platform::input::vk::to_vk_ext(code).is_some(),
                    "{id} 的 {code:?} 没有注入映射 —— 鼠标点它会毫无反应"
                );
            }
        }
    }

    /// ★ 端到端：发布真实几何后，点**每一颗**键帽的正中都能命中**那颗**键。
    ///
    /// 这条把"用户报的点了没反应"整条路径一次钉住：几何 → 发布到共享区 → 钩子侧判定。
    /// 只测其中一段都会漏 —— 发布对了但坐标空间不对（键盘用虚拟桌面物理像素、钩子给的是
    /// MSLLHOOKSTRUCT 的 `pt`，两者必须同一空间）时，前面所有单测照样全绿，而实际点哪儿
    /// 都不中。多显示器（原点为负）尤其容易踩，这里用带负原点的虚拟桌面跑一遍。
    #[test]
    fn hit_finds_the_clicked_key_end_to_end() {
        // 命中区是进程级共享区：与用它家的其它用例串行（见 `hit::test_lock`）。
        let _g = crate::platform::input::hit::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in ["ansi_104", "ansi_60"] {
            for screen in [
                ScreenRect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                // 双屏且左屏为主：虚拟桌面原点为负。
                ScreenRect {
                    x: -1920,
                    y: -200,
                    w: 3840,
                    h: 1280,
                },
            ] {
                let mut c = cfg();
                c.virtual_keyboard.layout = id.to_string();
                let mut m = VirtualKeyboardModule::new(&c);
                m.set_screen(screen);
                m.publish_hit_region(&c.virtual_keyboard);

                let rects = m.layout_rects_with(
                    &c.virtual_keyboard,
                    &vk_config::VkConfig::load(&c.virtual_keyboard),
                );
                assert!(!rects.is_empty());
                for (code, r) in &rects {
                    let (cx, cy) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
                    assert_eq!(
                        crate::platform::input::hit::hit(cx, cy),
                        Some(*code),
                        "{id} @ {screen:?}：点键帽正中 {cx},{cy} 应命中 {code:?}，矩形 {r:?}"
                    );
                }
                // 键盘之外的点必须落空（否则会吞掉用户在别处的点击）。
                let outside = (screen.x - 50, screen.y - 50);
                assert_eq!(
                    crate::platform::input::hit::hit(outside.0, outside.1),
                    None,
                    "{id}：键盘之外的点不该被吞"
                );
            }
        }
        // 复原全局状态，别影响同进程的其它用例。
        crate::platform::input::hit::publish(Vec::new(), false);
    }

    /// 命中区只在"键盘开着且没勾穿透"时活跃：勾上穿透后必须发布非活跃标记，
    /// 否则鼠标点键帽仍会被钩子吞掉，而用户以为已经变成纯展示了。
    #[test]
    fn hit_region_follows_enable_and_click_through() {
        let _g = crate::platform::input::hit::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut c = cfg();
        let m = VirtualKeyboardModule::new(&c);
        let s = &mut c.virtual_keyboard;

        m.publish_hit_region(s);
        assert!(
            crate::platform::input::hit::is_active(),
            "开着且没穿透 → 应可点"
        );

        s.click_through = true;
        m.publish_hit_region(s);
        assert!(
            !crate::platform::input::hit::is_active(),
            "勾了穿透 → 不该再吞点击"
        );

        s.click_through = false;
        s.enabled = false;
        m.publish_hit_region(s);
        assert!(
            !crate::platform::input::hit::is_active(),
            "关掉功能 → 不该再吞点击"
        );

        // 复原（本用例与 hits.rs 的用例同进程，别把全局状态留给别人）。
        s.enabled = true;
        m.publish_hit_region(s);
    }

    // ── 描边四边等厚（2026-09-26 用户报告）─────────────────────────────

    /// ★ 回归锁：键帽矩形必须落在**整数物理像素**上。
    ///
    /// 用户实测的现象是"右侧和上方描边 2px、左侧和下方 1px，而且不同键帽变厚的边不一样"。
    /// 直接成因是描边墨迹压在半像素上被抗锯齿摊开，而**半像素来自偏移的百分比**：
    /// 水平偏移 0.5% × 屏宽 1921 = 9.605，`x0` 从此带上小数尾巴，每颗键都跟着偏。
    ///
    /// 对旧代码是真红：没有吸附时 `x0 + x_u*step` 的小数尾巴会一直传到 `KeyBox::rect`。
    #[test]
    fn key_rects_snap_to_whole_pixels_with_a_fractional_offset() {
        let mut c = cfg();
        // 奇数屏宽 + 0.5% 偏移 → x0 必然带小数（1921 * 0.005 = 9.605）。
        c.virtual_keyboard.right_offset = 0.5;
        c.virtual_keyboard.bottom_offset = 5.0;
        let mut m = VirtualKeyboardModule::new(&c);
        m.set_screen(ScreenRect {
            x: 0,
            y: 0,
            w: 1921,
            h: 1080,
        });
        let boxes = m.layout_boxes(&c.virtual_keyboard);
        assert!(!boxes.is_empty(), "104 方案应算出键位");
        for b in &boxes {
            let r = b.rect;
            assert_eq!(r.x.fract(), 0.0, "键 {:?} 的 x 未吸附: {}", b.key.code, r.x);
            assert_eq!(r.y.fract(), 0.0, "键 {:?} 的 y 未吸附: {}", b.key.code, r.y);
            assert_eq!(r.w.fract(), 0.0, "键 {:?} 的 w 未吸附: {}", b.key.code, r.w);
            assert_eq!(r.h.fract(), 0.0, "键 {:?} 的 h 未吸附: {}", b.key.code, r.h);
            assert!(r.w >= 1.0 && r.h >= 1.0, "吸附后不该被压没: {r:?}");
        }
    }

    /// ★ 回归锁：整数矩形 + 整数笔宽 ⇒ 描边墨迹正好是**最外一圈像素**，四边等厚。
    ///
    /// 判据就是那句人话："墨迹左缘 = 键帽左缘，墨迹右缘 = 键帽右缘，厚度 = 笔宽"。
    /// 旧代码笔宽 1.5 时这一圈必然跨到第二行（那半行往哪边摊不等），四边厚度就不一样。
    #[test]
    fn cap_border_is_exactly_one_pixel_ring_on_all_four_sides() {
        let r = Rect::new(10.0, 20.0, 32.0, 32.0);
        let (stroke, w, fill) = cap_rects(r);
        assert_eq!(w, 1.0, "笔宽必须是 1 整像素");
        let hw = w * 0.5;
        // 四边的墨迹外缘 == 键帽边缘，厚度 == 笔宽（上下左右一式一份，不多不少）。
        assert_eq!(stroke.x - hw, r.x, "左缘");
        assert_eq!(stroke.y - hw, r.y, "上缘");
        assert_eq!(stroke.x + stroke.w + hw, r.x + r.w, "右缘");
        assert_eq!(stroke.y + stroke.h + hw, r.y + r.h, "下缘");
        assert_eq!(stroke.w + w, r.w, "横向墨迹总跨度 = 键宽");
        assert_eq!(stroke.h + w, r.h, "纵向墨迹总跨度 = 键高");
        // 填色接在描边内缘：不重叠（半透明描边不会叠深）、不留缝（不漏背景）。
        assert_eq!(fill.x, r.x + w);
        assert_eq!(fill.y, r.y + w);
        assert_eq!(fill.x + fill.w, r.x + r.w - w);
        assert_eq!(fill.y + fill.h, r.y + r.h - w);
    }
}
