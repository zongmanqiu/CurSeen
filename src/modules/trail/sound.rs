//! 点击音效的"按按钮选声 + 发声"。对齐 `trail/controller.py:208-242`。
//!
//! 与 `TrailModule` 分开是为了两件事可单测：**按按钮挑配置**（在 `settings` 里，纯函数、
//! 无 IO）与**真的发声**（要 OS 调用）。后者只经 `crate::platform::audio`，
//! 模块层因此不碰 `windows::`。
//!
//! ★ 与 1.0 的差别：1.0 只认左/右键（配置里只有 `sound_left_*` / `sound_right_*`），中键和
//! 侧键恒不出声。2.0 起五键各自独立（`sound_bindings`），选择逻辑统一收在
//! [`TrailSettings::sound_for_button`]。

use crate::core::events::MouseButton;
use crate::platform::audio;

use super::settings::TrailSettings;

/// 按按钮播声。返回是否真的发起了播放。
///
/// "谁该出声"完全由 [`TrailSettings::sound_for_button`] 决定（未启用 / 没选文件 → 静默）。
pub fn play_for_button(settings: &TrailSettings, button: MouseButton) -> bool {
    match settings.sound_for_button(&button) {
        Some(s) => audio::play(&s.file, u32::from(s.volume)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::trail::settings::ButtonSound;

    fn settings_with(pairs: &[(&str, bool, &str, u8)]) -> TrailSettings {
        let mut s = TrailSettings::default();
        for (key, enabled, file, volume) in pairs {
            s.sound_bindings.insert(
                (*key).to_string(),
                ButtonSound {
                    enabled: *enabled,
                    file: (*file).to_string(),
                    volume: *volume,
                },
            );
        }
        s
    }

    #[test]
    fn disabled_button_yields_nothing() {
        let s = TrailSettings::default(); // 五键全关
        assert!(s.sound_for_button(&MouseButton::Left).is_none());
        assert!(!play_for_button(&s, MouseButton::Left));
    }

    #[test]
    fn each_button_picks_its_own_sound() {
        let s = settings_with(&[
            ("left", true, "click1", 50),
            ("right", false, "switch13", 50),
            ("middle", true, "rollover2", 30),
            ("x1", true, "click5", 10),
        ]);
        assert_eq!(
            s.sound_for_button(&MouseButton::Left).unwrap().file,
            "click1"
        );
        assert!(
            s.sound_for_button(&MouseButton::Right).is_none(),
            "右键关着"
        );
        // 1.0 时代中键 / 侧键恒不出声；新模型下一视同仁。
        assert_eq!(
            s.sound_for_button(&MouseButton::Middle).unwrap().file,
            "rollover2"
        );
        assert_eq!(s.sound_for_button(&MouseButton::X1).unwrap().file, "click5");
        assert!(s.sound_for_button(&MouseButton::X2).is_none(), "侧键2 没配");
    }

    #[test]
    fn enabled_but_no_file_is_silent() {
        // 开关开着但声音名是空串（目录里一个 wav 都没有）→ 不发声，也不 panic。
        let s = settings_with(&[("left", true, "", 50)]);
        assert!(s.sound_for_button(&MouseButton::Left).is_none());
        assert!(!play_for_button(&s, MouseButton::Left));
    }

    #[test]
    fn volume_zero_is_silent() {
        // 不实际发声：音量 0 在 `audio::play` 里就返回 false。
        let s = settings_with(&[("left", true, "click3", 0)]);
        assert!(s.sound_for_button(&MouseButton::Left).is_some(), "选中了");
        assert!(!play_for_button(&s, MouseButton::Left), "但音量为 0");
    }

    #[test]
    fn volume_above_100_is_clamped_by_the_audio_layer() {
        let s = settings_with(&[("left", true, "click3", 100)]);
        assert_eq!(s.sound_for_button(&MouseButton::Left).unwrap().volume, 100);
    }
}
