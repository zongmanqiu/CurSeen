//! 击键回显模块。记录用户按键历史，在屏幕上显示按键弹窗。

pub mod controller;
pub mod keymap;
pub mod keynames;
pub mod layout;
pub mod merge;
pub mod render;
pub mod settings;

use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::InputEvent;
use crate::core::geometry::Size;
use crate::core::tick::Tick;
use crate::modules::Module;
use crate::platform::display::ScreenRect;

use settings::{from_config, KeyDisplaySettings};

/// 击键回显模块。
pub struct KeyDisplayModule {
    settings: KeyDisplaySettings,
    controller: controller::KeyController,
    last_config_version: u64,
    /// 虚拟桌面矩形（物理像素）。由 Runtime 每帧注入（P3.1 坐标对齐）。
    screen: ScreenRect,
    /// 挂钟起点。键盘弹窗的时间戳/淡出判定都基于真实时间，**不能用帧时钟**
    /// （`Tick::elapsed_ms` 只在出帧时累加，空闲停帧期间它不走 → 弹窗永远不会开始淡出）。
    clock_start: std::time::Instant,
}

impl KeyDisplayModule {
    pub fn new(config: &Config) -> Self {
        let settings = from_config(&config.key_display);
        let controller = controller::KeyController::new(&settings);
        Self {
            settings,
            controller,
            last_config_version: 0,
            screen: ScreenRect::FALLBACK,
            clock_start: std::time::Instant::now(),
        }
    }

    /// 注入当前虚拟桌面矩形（由 Runtime 每帧调用）。替换硬编码 1920×1080。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
    }

    /// 当前挂钟时间（秒）。与 [`Self::next_change_ms`] 配套使用。
    pub fn now_s(&self) -> f64 {
        self.clock_start.elapsed().as_secs_f64()
    }

    /// 下一次画面会变的时刻距现在多少毫秒（见 `KeyController::next_change_ms`）。
    pub fn next_change_ms(&self) -> Option<u32> {
        self.controller.next_change_ms(self.now_s())
    }

    /// 同步配置。
    fn sync_settings(&mut self, config: &Config, config_version: u64) {
        if config_version == self.last_config_version {
            return;
        }
        self.last_config_version = config_version;
        self.settings = from_config(&config.key_display);
        // 淡出延迟是运行时可调项：面板一改就得让控制器看到，否则一直用出厂值。
        self.controller.fade_delay = self.settings.fade_delay;
    }
}

impl Module for KeyDisplayModule {
    fn enabled(&self, config: &Config) -> bool {
        // ★ 开关架构：唯一开关是本功能段自己的 `enabled`（功能总开关已废除）。
        config.key_display.enabled
    }

    fn tick(&mut self, tick: &Tick, config: &Config, input: &Option<InputEvent>) {
        let dt_s = tick.dt_ms / 1000.0;
        // ★ 秒（对齐 Python 的 `time.time()`），不是毫秒。
        let now = self.now_s();

        self.sync_settings(config, self.last_config_version + 1);

        // ★ 关着就既不记录也不推进，并把状态清空。此前控制器永远 `enabled == true`，
        //   只靠 `draw()` 早退兜底 —— "关掉击键回显、开着拖尾"期间按键照记，重新打开时
        //   会看到一堆几分钟前按过的键。与虚拟键盘 `tick` 的"关掉即清"同款。
        //   只在确实有残留时才清，避免每帧空转。
        if !config.key_display.enabled {
            if self.controller.active.is_some()
                || !self.controller.items.is_empty()
                || !self.controller.mods.is_empty()
            {
                self.controller.clear();
            }
            self.controller.enabled = false;
            return;
        }
        self.controller.enabled = true;

        // 处理输入事件。
        //
        // ★ 两条事件**都必须先 `sync_mods`**：本地修饰键状态只由"修饰键自己的
        //   KeyDown / KeyUp"维护，键程是两条事件 —— 丢一条就留下一个幽灵修饰键，
        //   表现为"单字母被显示成 Shift + 字母、且每个字母单独一个弹窗"。
        //   事件自带的 `Modifiers` 由钩子的原子缓存算出、每条都带，是权威且自校正的。
        match input {
            Some(InputEvent::KeyDown { key, mods, .. }) => {
                self.controller.sync_mods(mods, now);
                if let Some(vk) = key_to_vk(key) {
                    self.controller.on_key_pressed(vk, now, &self.settings);
                }
            }
            // ★ `KeyUp` 以前**完全没被处理**（`on_key_released` 只有测试在调）→ 修饰键
            //   永远处于"按着"状态，只能等 5 秒的 `MOD_STUCK_TIMEOUT` 兜底清掉。
            //   这是幽灵修饰键的主因，也是"Shift + 字母"最直接的来源。
            Some(InputEvent::KeyUp { key, mods, .. }) => {
                self.controller.sync_mods(mods, now);
                if let Some(vk) = key_to_vk(key) {
                    self.controller.on_key_released(vk);
                }
            }
            // ★ 鼠标按键回显（2026-09-22 用户拍板恢复）：既然叫"回显"，鼠标也是输入——
            //   左键 / 右键 / 中键 / 侧键1·2 走 MouseDown，滚轮上·下·左·右走 MouseWheel。
            //   ⚠️ 普通滚轮默认**不转发**（钩子只转发 Ctrl+Win+滚轮 给放大镜缩放），
            //   要回显就得由 `mouse_echo_armed` 那道门放行，见
            //   `platform::input::mouse::set_mouse_echo_armed` 与 `Runtime::sync_input_gate`。
            // 两条鼠标事件都带 `if self.settings.show_mouse` 守卫：**守卫不过 = 落到
            // `_ => {}` 丢掉这条事件**（正是想要的语义，不是"留给后面的分支"）。
            Some(InputEvent::MouseDown { button, .. }) if self.settings.show_mouse => {
                let (id, builtin) = keynames::mouse_ident(*button);
                self.controller
                    .on_mouse_pressed(id, builtin, now, &self.settings);
            }
            Some(InputEvent::MouseWheel {
                delta, horizontal, ..
            }) if self.settings.show_mouse => {
                // 与 Windows 滚轮同向：前推（delta>0）是上滚；倾斜右移（delta>0）是右滚。
                let (id, builtin) = keynames::wheel_ident(*horizontal, *delta > 0);
                self.controller
                    .on_mouse_pressed(id, builtin, now, &self.settings);
            }
            _ => {}
        }

        // 推进动画
        self.controller.tick(dt_s, now);
    }

    fn draw(&self, list: &mut DrawList, config: &Config) {
        if !config.key_display.enabled {
            return;
        }
        let items: Vec<_> = self.controller.items.items().cloned().collect();
        if items.is_empty() && self.controller.active.is_none() {
            return;
        }

        let mut all_items = items;
        if let Some(ref active) = self.controller.active {
            all_items.push(active.clone());
        }

        let settings = from_config(&config.key_display);
        let screen = Size {
            w: self.screen.w as f32,
            h: self.screen.h as f32,
        };
        let mut metrics = FakeMetrics;
        let boxes = layout::layout(&all_items, &settings, screen, &mut metrics);
        render::render_key_popups(&boxes, &settings, list, settings.font_size);
    }
}

/// KeyCode 转 VK 码（简单映射）。
fn key_to_vk(key: &crate::core::events::KeyCode) -> Option<u16> {
    use crate::core::events::KeyCode;
    match key {
        KeyCode::Char(c) => {
            let ch = *c;
            if ch.is_ascii() && ch.is_alphanumeric() {
                Some(ch.to_ascii_uppercase() as u16)
            } else if ch == ';' {
                Some(0xBA)
            } else if ch == '=' {
                Some(0xBB)
            } else if ch == ',' {
                Some(0xBC)
            } else if ch == '-' {
                Some(0xBD)
            } else if ch == '.' {
                Some(0xBE)
            } else if ch == '/' {
                Some(0xBF)
            } else if ch == '`' {
                Some(0xC0)
            } else if ch == '[' {
                Some(0xDB)
            } else if ch == '\\' {
                Some(0xDC)
            } else if ch == ']' {
                Some(0xDD)
            } else if ch == '\'' {
                Some(0xDE)
            } else if ch == ' ' {
                Some(0x20)
            } else {
                None
            }
        }
        KeyCode::F(n) => Some(0x6F + *n as u16),
        KeyCode::Esc => Some(0x1B),
        KeyCode::Tab => Some(0x09),
        KeyCode::CapsLock => Some(0x14),
        KeyCode::Backspace => Some(0x08),
        KeyCode::Enter => Some(0x0D),
        KeyCode::Space => Some(0x20),
        KeyCode::LShift | KeyCode::RShift => Some(0x10),
        KeyCode::LCtrl | KeyCode::RCtrl => Some(0x11),
        KeyCode::LAlt | KeyCode::RAlt => Some(0x12),
        KeyCode::LWin | KeyCode::RWin => Some(0x5B),
        KeyCode::Menu => Some(0x5D),
        KeyCode::PrtSc => Some(0x2C),
        KeyCode::ScrollLock => Some(0x91),
        KeyCode::Pause => Some(0x13),
        KeyCode::Insert => Some(0x2D),
        KeyCode::Delete => Some(0x2E),
        KeyCode::Home => Some(0x24),
        KeyCode::End => Some(0x23),
        KeyCode::PageUp => Some(0x21),
        KeyCode::PageDown => Some(0x22),
        KeyCode::Up => Some(0x26),
        KeyCode::Down => Some(0x28),
        KeyCode::Left => Some(0x25),
        KeyCode::Right => Some(0x27),
        KeyCode::NumAdd => Some(0x6B),
        KeyCode::NumSub => Some(0x6D),
        KeyCode::NumMul => Some(0x6A),
        KeyCode::NumDiv => Some(0x6F),
        KeyCode::NumDot => Some(0x6E),
        // ★ 小键盘数字 0-9 与 NumLock（2026-09-22 用户实测缺失修复）：
        // 钩子把 VK 0x60-0x69 转成 KeyCode::Num(0-9)、0x90 转成 NumLock，
        // 但这里漏了两个分支 → 落入 _ => None → 整键不显示
        // （小键盘运算符有专门分支所以正常，与用户观察完全一致）。
        KeyCode::Num(n) if *n <= 9 => Some(0x60 + *n as u16),
        KeyCode::NumEnter => Some(0x6C),
        KeyCode::NumLock => Some(0x90),
        _ => None,
    }
}

/// 测试用假度量器。
struct FakeMetrics;

impl layout::TextMeasure for FakeMetrics {
    fn measure(&mut self, s: &str, size_pt: f32) -> layout::TextMetrics {
        // 与 render::advance / layout tests 同一张字宽表（2026-09-22 统一）。
        layout::TextMetrics {
            width: layout::text_advance(s, size_pt),
            ascent: size_pt * 0.8,
            height: size_pt * 1.2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::events::KeyCode;

    fn make_module() -> KeyDisplayModule {
        let config = Config::default();
        let mut m = KeyDisplayModule::new(&config);
        // ★ 测试与盘上 ini 解耦：指向一个不存在的路径 → refresh 后覆盖表为空，
        //   全部走内置默认名。真实 ini 是**用户可自定义**的（如 MouseLeft = LMB），
        //   模块级测试断言的是内置名，绝不能随用户当前的自定义漂移。
        //   需要 ini 覆盖的用例（count_badge / mouse_left_uses 等）再显式替换字段。
        m.controller.keynames =
            keynames::Keynames::new(std::env::temp_dir().join("curseen_absent_keynames.ini"));
        m
    }

    /// 功能**开启**的配置（`Config::default()` 里 `key_display.enabled == false`）。
    fn enabled_config() -> Config {
        let mut c = Config::default();
        c.key_display.enabled = true;
        c
    }

    /// ★ 小键盘映射全覆盖（2026-09-22 修复回归锁）：数字 0-9、NumLock、NumEnter
    /// 此前在 key_to_vk 里落入 _ => None，整键不显示（用户实测）。
    #[test]
    fn numpad_and_numlock_map_to_vk() {
        use crate::core::events::KeyCode;

        // 数字 0-9（Num(0-9) → VK 0x60-0x69）
        for n in 0..=9u8 {
            assert_eq!(
                key_to_vk(&KeyCode::Num(n)),
                Some(0x60 + n as u16),
                "Num{n} 应映射到 VK 0x{:02X}",
                0x60 + n
            );
        }
        // NumLock
        assert_eq!(key_to_vk(&KeyCode::NumLock), Some(0x90));
        // 小键盘 Enter（修正既有错映射 Num(0) 后归位）
        assert_eq!(key_to_vk(&KeyCode::NumEnter), Some(0x6C));
        // 运算符与小数点不受影响
        assert_eq!(key_to_vk(&KeyCode::NumAdd), Some(0x6B));
        assert_eq!(key_to_vk(&KeyCode::NumDot), Some(0x6E));
    }

    #[test]
    fn char_key() {
        let mut m = make_module();
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        let input = InputEvent::KeyDown {
            key: KeyCode::Char('A'),
            mods: crate::core::events::Modifiers::default(),
            repeated: false,
            time_ms: 1000,
        };
        m.tick(&tick, &config, &Some(input));
        assert!(m.controller.active.is_some());
        assert_eq!(m.controller.active.as_ref().unwrap().display_text(), "A");
    }

    #[test]
    fn ctrl_shortcut() {
        let mut m = make_module();
        let config = enabled_config();
        let tick1 = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        let input1 = InputEvent::KeyDown {
            key: KeyCode::LCtrl,
            mods: crate::core::events::Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeated: false,
            time_ms: 1000,
        };
        m.tick(&tick1, &config, &Some(input1));
        let tick2 = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 2.0,
        };
        let input2 = InputEvent::KeyDown {
            key: KeyCode::Char('C'),
            mods: crate::core::events::Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeated: false,
            time_ms: 2000,
        };
        m.tick(&tick2, &config, &Some(input2));
        let text = m.controller.active.as_ref().unwrap().display_text();
        assert!(text.contains("Ctrl"));
    }

    #[test]
    fn draw_empty() {
        let m = make_module();
        let config = Config::default();
        let mut list = DrawList::new();
        m.draw(&mut list, &config);
        assert!(list.cmds().is_empty());
    }

    #[test]
    fn tick_empty() {
        let mut m = make_module();
        let config = Config::default();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(&tick, &config, &None);
    }

    /// ★ 端到端：`Shift` 的 KeyUp 必须被**消费掉**。
    ///
    /// 回归锚点（用户报「打英文单字符总是显示 Shift + 字母」）：模块的 `tick` 曾经只
    /// `match InputEvent::KeyDown`，`on_key_released` 除测试外**没有任何调用点** —— 修饰键
    /// 永远停在"按着"，只能等 `MOD_STUCK_TIMEOUT`（5 秒）兜底清掉。
    #[test]
    fn key_up_releases_modifier_end_to_end() {
        let mut m = make_module();
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        let shift = crate::core::events::Modifiers {
            shift: true,
            ..Default::default()
        };

        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyDown {
                key: KeyCode::LShift,
                mods: shift,
                repeated: false,
                time_ms: 1000,
            }),
        );
        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyDown {
                key: KeyCode::Char('A'),
                mods: shift,
                repeated: false,
                time_ms: 1100,
            }),
        );
        let text = m.controller.active.as_ref().unwrap().display_text();
        assert!(
            text.contains("Shift"),
            "按住 Shift 打 A 应当是快捷键：{text}"
        );

        // 松手。事件里不带任何修饰键。
        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyUp {
                key: KeyCode::LShift,
                mods: crate::core::events::Modifiers::default(),
                time_ms: 1200,
            }),
        );
        assert_ne!(
            m.controller.mods.get("Shift"),
            Some(&true),
            "KeyUp 必须真的被消费掉（此前 on_key_released 根本没被调用）"
        );

        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyDown {
                key: KeyCode::Char('B'),
                mods: crate::core::events::Modifiers::default(),
                repeated: false,
                time_ms: 1300,
            }),
        );
        assert_eq!(
            m.controller.active.as_ref().unwrap().display_text(),
            "B",
            "松开 Mod 后的字母必须是纯键"
        );
    }

    /// ★ 功能关着时：不记录，并且把残留清掉（重新打开不该冒出几分钟前的旧键）。
    #[test]
    fn disabled_feature_records_nothing_and_clears_residue() {
        let mut m = make_module();
        let on = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(
            &tick,
            &on,
            &Some(InputEvent::KeyDown {
                key: KeyCode::Char('A'),
                mods: crate::core::events::Modifiers::default(),
                repeated: false,
                time_ms: 1000,
            }),
        );
        assert!(m.controller.active.is_some());

        m.tick(
            &tick,
            &Config::default(), // key_display.enabled == false
            &Some(InputEvent::KeyDown {
                key: KeyCode::Char('B'),
                mods: crate::core::events::Modifiers::default(),
                repeated: false,
                time_ms: 1300,
            }),
        );
        assert!(
            m.controller.active.is_none() && m.controller.items.is_empty(),
            "关掉后既不该记录新键，也不该留着旧键"
        );
        assert!(
            m.controller.next_change_ms(m.now_s()).is_none(),
            "清空后不该再排帧（否则关掉功能也一直唤醒 UI）"
        );
    }

    /// ★ 鼠标按键回显（2026-09-22 恢复）：MouseDown 应当像按键一样进弹窗（同一条
    /// `push_display` 合流），左/右/中/侧键逐一可区分。
    #[test]
    fn mouse_down_shows_a_popup() {
        use crate::core::events::MouseButton;

        let mut m = make_module();
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };

        for (button, expected) in [
            (MouseButton::Left, "左键"),
            (MouseButton::Right, "右键"),
            (MouseButton::Middle, "中键"),
            (MouseButton::X1, "侧键1"),
            (MouseButton::X2, "侧键2"),
        ] {
            m.controller.clear();
            m.tick(
                &tick,
                &config,
                &Some(InputEvent::MouseDown {
                    button,
                    x: 100,
                    y: 100,
                    time_ms: 1000,
                }),
            );
            let text = m.controller.active.as_ref().map(|a| a.display_text());
            assert_eq!(text.as_deref(), Some(expected), "{expected} 按下必须出弹窗");
        }
    }

    /// 滚轮两个方向（含横向倾斜）都要回显，且方向不能反（`delta > 0` = 上/右）。
    #[test]
    fn mouse_wheel_reports_its_direction() {
        let mut m = make_module();
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };

        for (delta, horizontal, expected) in [
            (120, false, "滚轮上"),
            (-120, false, "滚轮下"),
            (120, true, "滚轮右"),
            (-120, true, "滚轮左"),
        ] {
            m.controller.clear();
            m.tick(
                &tick,
                &config,
                &Some(InputEvent::MouseWheel {
                    delta,
                    horizontal,
                    x: 100,
                    y: 100,
                    time_ms: 1000,
                }),
            );
            let text = m.controller.active.as_ref().map(|a| a.display_text());
            assert_eq!(text.as_deref(), Some(expected), "滚轮方向显示错");
        }
    }

    /// ★ 计数徽标格式统一（2026-09-22 用户口径）：所有击键（键盘/鼠标）的重复计数
    /// 都是「名 空格 x 次数」（"C x5" / "LMB x5"）——x 与数字之间无空格、与名字之间
    /// 有空格。键盘与鼠标走同一条 merge/display_text，本用例把两条路径钉在一起
    /// （含合流进同一弹窗的真实场景：先按 C×5 再点 LMB×5，鼠标组合入键盘条目）。
    #[test]
    fn count_badge_format_is_uniform_for_keyboard_and_mouse() {
        use crate::core::events::{InputEvent, KeyCode, MouseButton};

        let mut m = make_module();
        let path =
            std::env::temp_dir().join(format!("curseen_count_fmt_{}.ini", std::process::id()));
        std::fs::write(&path, "MouseLeft = LMB\n").unwrap();
        m.controller.keynames = keynames::Keynames::new(path.clone());
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };

        for i in 0..5 {
            m.tick(
                &tick,
                &config,
                &Some(InputEvent::KeyDown {
                    key: KeyCode::Char('C'),
                    mods: crate::core::events::Modifiers::default(),
                    repeated: false,
                    time_ms: 1000 + i * 100,
                }),
            );
        }
        assert_eq!(
            m.controller
                .active
                .as_ref()
                .unwrap()
                .display_text()
                .trim_end(),
            "C x5",
            "键盘重复计数 = 名 空格 x 次数"
        );

        for i in 0..5 {
            m.tick(
                &tick,
                &config,
                &Some(InputEvent::MouseDown {
                    button: MouseButton::Left,
                    x: 0,
                    y: 0,
                    time_ms: 2000 + i * 100,
                }),
            );
        }
        let text = m.controller.active.as_ref().unwrap().display_text();
        assert!(
            text.ends_with(" LMB x5"),
            "鼠标重复计数与键盘同格式（合流进同一条目）：{text}"
        );
        assert!(
            !text.contains("x 5") && !text.contains("LMBx"),
            "旧格式残留：{text}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// (ID, 内置名) 穷举：五个鼠标键 + 滚轮四向全在册（映射不允许漏分支）。
    #[test]
    fn mouse_idents_cover_all_nine() {
        use crate::core::events::MouseButton;

        assert_eq!(
            keynames::mouse_ident(MouseButton::Left),
            ("MouseLeft", "左键")
        );
        assert_eq!(
            keynames::mouse_ident(MouseButton::Right),
            ("MouseRight", "右键")
        );
        assert_eq!(
            keynames::mouse_ident(MouseButton::Middle),
            ("MouseMiddle", "中键")
        );
        assert_eq!(keynames::mouse_ident(MouseButton::X1), ("MouseX1", "侧键1"));
        assert_eq!(keynames::mouse_ident(MouseButton::X2), ("MouseX2", "侧键2"));
        assert_eq!(keynames::wheel_ident(false, true), ("WheelUp", "滚轮上"));
        assert_eq!(keynames::wheel_ident(false, false), ("WheelDown", "滚轮下"));
        assert_eq!(keynames::wheel_ident(true, true), ("WheelRight", "滚轮右"));
        assert_eq!(keynames::wheel_ident(true, false), ("WheelLeft", "滚轮左"));
    }

    /// ★ ini 覆盖对鼠标同样生效：把 MouseLeft 改名后，弹窗显示新文本。
    #[test]
    fn mouse_left_uses_ini_override_text() {
        use crate::core::events::MouseButton;

        let mut m = make_module();
        let path =
            std::env::temp_dir().join(format!("curseen_mouse_ident_{}.ini", std::process::id()));
        std::fs::write(&path, "MouseLeft = 左击\n").unwrap();
        m.controller.keynames = keynames::Keynames::new(path.clone());
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        m.tick(
            &tick,
            &config,
            &Some(InputEvent::MouseDown {
                button: MouseButton::Left,
                x: 0,
                y: 0,
                time_ms: 1000,
            }),
        );
        let text = m.controller.active.as_ref().unwrap().display_text();
        assert_eq!(text, "左击", "ini 覆盖必须生效（默认文本是左键）");
        let _ = std::fs::remove_file(&path);
    }

    /// 带修饰键的鼠标组合显示前缀（Ctrl + 左键），与按键同款。
    #[test]
    fn mouse_click_with_modifier_shows_prefix() {
        use crate::core::events::MouseButton;

        let mut m = make_module();
        let config = enabled_config();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };
        let ctrl = crate::core::events::Modifiers {
            ctrl: true,
            ..Default::default()
        };

        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyDown {
                key: KeyCode::LCtrl,
                mods: ctrl,
                repeated: false,
                time_ms: 1000,
            }),
        );
        m.tick(
            &tick,
            &config,
            &Some(InputEvent::MouseDown {
                button: MouseButton::Left,
                x: 100,
                y: 100,
                time_ms: 1100,
            }),
        );
        let text = m.controller.active.as_ref().unwrap().display_text();
        assert!(
            text.contains("Ctrl") && text.contains("左键"),
            "应显示 Ctrl + 左键：{text}"
        );
    }

    /// ★ 「排除鼠标」模式（show_mouse 派生为 false）下：鼠标四类事件都不产生弹窗、
    /// 不记历史、不排帧；键盘不受牵连。守卫不过 = 落到 `_ => {}` 丢事件（想要的语义）。
    /// （2026-09-25 起独立开关已删，show_mouse 由 display_mode 派生——本测试改用
    ///   display_mode = "no_mouse" 驱动同一条守卫；tick 每次都会从配置重建 settings。）
    #[test]
    fn show_mouse_off_suppresses_mouse_popups() {
        use crate::core::events::MouseButton;

        let mut m = make_module();
        let mut config = enabled_config();
        config.key_display.display_mode = "no_mouse".into();
        let tick = Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 1.0,
        };

        for input in [
            InputEvent::MouseDown {
                button: MouseButton::Left,
                x: 100,
                y: 100,
                time_ms: 1000,
            },
            InputEvent::MouseUp {
                button: MouseButton::Right,
                x: 100,
                y: 100,
                time_ms: 1010,
            },
            InputEvent::MouseWheel {
                delta: 120,
                horizontal: false,
                x: 100,
                y: 100,
                time_ms: 1020,
            },
            InputEvent::MouseMove {
                x: 120,
                y: 130,
                time_ms: 1030,
            },
        ] {
            m.tick(&tick, &config, &Some(input));
            assert!(m.controller.active.is_none(), "鼠标事件跑出了弹窗");
            assert!(m.controller.items.is_empty(), "鼠标事件被记进了历史项");
            assert!(
                m.controller.next_change_ms(m.now_s()).is_none(),
                "鼠标事件不该排帧（否则一个键都不按也要每帧唤醒 UI）"
            );
        }

        m.tick(
            &tick,
            &config,
            &Some(InputEvent::KeyDown {
                key: KeyCode::Char('A'),
                mods: crate::core::events::Modifiers::default(),
                repeated: false,
                time_ms: 1100,
            }),
        );
        assert!(
            m.controller.active.is_some(),
            "鼠标事件之后键盘仍应照常产生弹窗"
        );
    }
}
