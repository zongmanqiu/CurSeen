//! 击键音效模块：每次按键（KeyDown）按 Lua 脚本选定的音效发声。
//!
//! # 脚本契约（统一 Lua 框架，assets/lua/ 下预设脚本头注释是权威版本）
//!
//! 脚本放在 `assets/lua/<id>.lua`，声明 `role = "keysound"` + `run(ctx, params)`，宿主
//! 经 `EngineCache` 执行。每次按键打包的 `ctx` 见 [`crate::core::config::KeySoundSettings`]
//! 的文档：`{ role, sounds, key, mods={ctrl,alt,shift,win}, repeated, dt }` + 既有 `fs`/`ext`。
//!
//! # 音源（2026-09-25 二次改版：浏览即引用，无中转素材目录）
//!
//! 脚本参数里的 `file` 浏览按钮选音频后**直接记路径**——程序目录内的记便携相对
//! 路径，外部文件记绝对路径，不复制、不落中转目录。`ctx.sounds` = 出厂素材目录
//! `assets/Sounds/` 的音频文件名列表（排序），供脚本按索引取出厂音。
//! 宿主经 [`stored_file_path`] 的解析链（绝对 / 便携相对 / Sounds）找到真实
//! 文件播放；脚本不调 `fs` 读音频。
//!
//! ## 返回值语义
//! - 素材文件名（或 `ctx.sounds` 的**索引**，1 起）→ 播放它；
//! - `false` → **强制静音**（连默认音也不响，供"长按不连发出声"这类开关）；
//! - `nil` / 空串 → 落回**默认音**（`resolve_stored_file`：配置 `file` 字段，
//!   出厂 `click3`；该字段不再有面板行，仅作兜底）。
//!
//! # 两层音源
//!
//! - **默认音**（配置 `file`，出厂 `click3`）：脚本返回 `nil` 时的回退。
//! - **Lua 脚本**：`run` 返回文件名/索引则按键覆盖默认；返回 `false` 强制静音。
//!   简单需求在脚本参数里挑一个文件即可，进阶的每键自定义才写脚本。
//!
//! # 复用与线程
//!
//! - 播放走 `platform::audio`（模块层不碰 `windows::`）。
//! - 键名与击键回显同源（`KeyCode::display_name`），用户写脚本时对着屏幕上的键名写。
//! - ★ **全键发声**（2026-09-21 用户反馈"键位不全"）：修饰键（Ctrl/Alt/Shift/Win）与
//!   普通键一视同仁。
//! - `EngineCache` 缓存每脚本长期 VM + mtime 热更新；都在 UI 线程执行，无锁。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mlua::Value;
use serde_json::Value as Json;

use crate::core::config::KeySoundSettings;
use crate::core::events::{KeyCode, Modifiers};
use crate::platform::audio;
use crate::scripting::runtime::EngineCache;

/// 扫音效素材目录，返回排序后的**文件名列表**（含扩展名，如 `{"a.wav","b.wav",…}`）。
///
/// 也就是要塞给 `ctx.sounds` 的元素集合。目录不存在或为空 → 空列表。
pub fn wav_filenames(dir: &str) -> Vec<String> {
    let dir_p = crate::core::paths::from_app_relative(dir);
    if dir_p.as_os_str().is_empty() {
        return Vec::new();
    }
    audio::list_audio(&dir_p)
        .into_iter()
        .map(|e| format!("{}.{}", e.stem, e.ext))
        .collect()
}

/// 「音效文件」行的值 → 绝对路径。
///
/// 解析顺序：绝对路径 → 便携相对（相对**程序目录**）→ `assets/Sounds/`
/// （出厂 `click3` 就在那儿）；每级按支持扩展名补全。
/// ★ 2026-09-25 二次改版：浏览即引用（配置直接记路径），中转目录
/// `assets/KeySounds/` 已退役；历史配置里的裸文件名仍由 Sounds 级兜住。
///
/// ★ 与 [`KeySoundModule::resolve_stored_file`] 是**同一套解析**（本函数唯二的播放端调用
/// 方之一）；面板「音效文件」行显示端也用它（`ui::sfx::sound_file_display`）——"显示的
/// 路径"和"实际响的文件"是两回事的大忌靠这一个函数杜绝。
pub fn stored_file_path(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let raw = Path::new(name);
    if raw.is_absolute() {
        return complete_ext(raw);
    }
    // 便携写法：相对**程序目录**（换台电脑仍有效，见 `paths::to_app_relative`）。
    let in_app = crate::core::paths::from_app_relative(name);
    if let Some(p) = complete_ext(&in_app) {
        return Some(p);
    }
    complete_ext(&crate::core::paths::sounds_dir().join(raw)).map(absolutize)
}

/// 按支持的扩展名补全：`x` → `x.wav` → `x.mp3` → …（首个真实存在的文件）。
fn complete_ext(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    for ext in audio::SUPPORTED_EXT {
        let c = p.with_extension(ext);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// ★ 相对路径 → 绝对路径（拼上当前工作目录）。见旧实现说明：`audio::play` 只认
/// 绝对路径或 Sounds 目录里的裸名，相对路径会静默丢弃，绝对化把不确定性钉死。
fn absolutize(p: PathBuf) -> PathBuf {
    if p.is_absolute() {
        return p;
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(p),
        Err(_) => p,
    }
}

/// `run(ctx, params)` 返回值的解释结果。
#[derive(Debug)]
enum SoundPick {
    /// 按键覆盖：用这个文件名发声（经 [`stored_file_path`] 解析成绝对路径）。
    File(String),
    /// 强制静音：连「音效文件」默认音也不响（脚本返回 `false`）。
    Silent,
}

/// 击键音效模块。
pub struct KeySoundModule {
    /// 当前脚本 id（与配置一致；变了即重置宿主缓存）。
    script: String,
    /// 「音效文件」行选中的声音名（便携相对 / 绝对路径，Sounds 兜底解析）。
    file: String,
    volume: u8,
    /// 脚本绝对路径（`lua_ext_dir()/<script>.lua`）。
    script_path: PathBuf,
    /// 当前脚本的参数现值（`config.key_sound.params[script]`；sync 时取好）。
    params: HashMap<String, Json>,
    /// 复用统一框架的宿主级引擎缓存（每脚本长期 VM + 热更新）。
    engine_cache: EngineCache,
    /// 素材目录（出厂 `assets/Sounds`）。`sounds` 的扫描基准；记成字符串
    /// 只为"没变就不重扫"的缓存判断。
    sound_dir: String,
    /// 缓存的素材文件名列表（= `ctx.sounds`；目录没变就不重扫）。
    sounds: Vec<String>,
    /// "脚本坏了/没有 run" 只打一次日志（每次按键都打会刷屏）。
    fault_logged: bool,
    /// 首次成功发声打一条 INFO（真机探针据此验证链路；后续静默，打字不刷日志）。
    first_play_logged: bool,
}

impl KeySoundModule {
    pub fn new() -> Self {
        Self {
            script: String::new(),
            file: String::new(),
            // 与点击音效同出厂值（100）；唯一来源是 `KeySoundSettings::default()`，
            // 这里不抄字面量——抄了就会出现"面板 100、实际 80"这种影子默认值。
            volume: KeySoundSettings::default().volume,
            script_path: PathBuf::new(),
            params: HashMap::new(),
            engine_cache: EngineCache::new(),
            sound_dir: String::new(),
            sounds: Vec::new(),
            fault_logged: false,
            first_play_logged: false,
        }
    }

    /// 与配置对账：脚本名 / 音量变了即生效（换脚本重置缓存，下次按键重载）。
    ///
    /// ★ `ctx.sounds` 恒扫出厂素材目录 `assets/Sounds/`（2026-09-25 二次改版：
    ///   浏览即引用、无中转目录）。目录内容在进程外也会变（用户丢文件进去），所以
    ///   这里**每次对账都重扫**——目录通常只有几个文件，扫描成本可忽略。
    pub fn sync(&mut self, s: &KeySoundSettings) {
        if self.script != s.script {
            self.script = s.script.clone();
            // 统一框架：脚本解析到 `assets/lua/<id>.lua`（2026-09-26 定稿：通用扩展
            // 脚本统一归档在 `scripts/` 文件夹，id = `scripts/<英文文件名>`）。
            self.script_path = crate::core::paths::lua_ext_dir().join(format!("{}.lua", s.script));
            self.engine_cache = EngineCache::new();
            self.fault_logged = false;
        }
        self.volume = s.volume;
        self.file = s.file.clone();
        // 当前脚本的参数段（面板按 schema 落盘；缺键由脚本 `params.x or 默认` 兜底）。
        self.params = s.params.get(&s.script).cloned().unwrap_or_default();
        let dir = crate::core::paths::sounds_dir()
            .to_string_lossy()
            .into_owned();
        if self.sound_dir != dir {
            self.sound_dir = dir;
            self.sounds = wav_filenames(&self.sound_dir);
        }
    }

    /// 按键（KeyDown）入口。返回是否真的发起了播放（测试与日志用）。
    pub fn on_key_down(&mut self, key: KeyCode, mods: Modifiers, repeated: bool) -> bool {
        // 第一层音源：脚本 `run(ctx,params)`（VM 未建/脚本坏/返回 nil 都落空，交给第二层）。
        // 执行出错时**回落到音效文件**而不是整键静默——用户至少还有声。
        let script_pick = match self.run_script(key, mods, repeated) {
            // 脚本明确要静音（返回 false）：不回落「音效文件」。
            Some(SoundPick::Silent) => return false,
            Some(SoundPick::File(fname)) => match self.wav_path(&fname) {
                Some(p) => Some(p),
                None => {
                    self.fault_once(&format!("音效文件不存在：{fname}（回落到默认音）"));
                    None
                }
            },
            None => None,
        };
        // 第二层音源：「音效文件」行（默认 Sounds 目录的 click3）。
        let Some(path) = script_pick.or_else(|| self.resolve_stored_file()) else {
            // 脚本没给、音效文件也没配——这一键静默。
            return false;
        };
        let played = audio::play(&path.to_string_lossy(), u32::from(self.volume));
        if played && !self.first_play_logged {
            log::info!(
                "击键音效：首次发声 {}（音量 {}%）——后续按键不再逐条记录",
                path.display(),
                self.volume
            );
            self.first_play_logged = true;
        }
        played
    }

    /// 调脚本 `run(ctx, params)` 并解释返回值。
    /// `None` = 落回「音效文件」（nil/空串/VM 未建/脚本坏/调用出错）；
    /// `Some(Silent)` = 脚本明确要静音；`Some(File(name))` = 覆盖（经 `stored_file_path` 解析）。
    fn run_script(&mut self, key: KeyCode, mods: Modifiers, repeated: bool) -> Option<SoundPick> {
        let params = self.params.clone();
        let sounds = self.sounds.clone();
        let key_name = key.display_name().to_string();
        let script_path = self.script_path.clone();

        // 构造 `ctx.sounds` / `ctx.mods` 需要 Lua 引用：先取缓存 VM 的 Lua（同一次按键内
        // mtime 不会变，不会触发热更新重建 VM → state 一致，能安全传值）。
        let fields: Vec<(&str, Value)> = {
            let lua = match self.engine_cache.lua(&script_path) {
                Ok(l) => l,
                Err(e) => {
                    self.fault_once(&format!("脚本加载失败：{e}"));
                    return None;
                }
            };
            let mut v: Vec<(&str, Value)> = Vec::with_capacity(6);
            if let Ok(t) = lua.create_table() {
                for (i, f) in sounds.iter().enumerate() {
                    let _ = t.set((i as i64) + 1, f.as_str());
                }
                v.push(("sounds", Value::Table(t)));
            }
            if let Ok(t) = lua.create_table() {
                let _ = t.set("ctrl", mods.ctrl);
                let _ = t.set("alt", mods.alt);
                let _ = t.set("shift", mods.shift);
                let _ = t.set("win", mods.win);
                v.push(("mods", Value::Table(t)));
            }
            if let Ok(s) = lua.create_string(&key_name) {
                v.push(("key", Value::String(s)));
            }
            v.push(("repeated", Value::Boolean(repeated)));
            v.push(("dt", Value::Integer(0)));
            v
        };

        let ret = self
            .engine_cache
            .run(&script_path, "keysound", &fields, &params);
        match ret {
            Ok(Value::Boolean(false)) => Some(SoundPick::Silent),
            Ok(Value::String(s)) => {
                let t = s.to_string_lossy().trim().to_string();
                if t.is_empty() {
                    None
                } else {
                    Some(SoundPick::File(t))
                }
            }
            // 索引语义（1 起）：返回第几个，越界/≤0 落回默认。
            Ok(Value::Integer(i)) => pick_index(i, &sounds),
            Ok(Value::Number(n)) => pick_index(n as i64, &sounds),
            _ => None,
        }
    }

    /// 脚本返回的文件名 → 绝对路径（文件存在才对）。
    ///
    /// ★ 走 [`stored_file_path`] 同一条解析链（绝对 / 便携相对 / Sounds 出厂）。
    ///   2026-09-25 二次改版后 `file` 参数直接记路径，绝对/相对都由链首两级对上。
    fn wav_path(&self, fname: &str) -> Option<PathBuf> {
        stored_file_path(fname)
    }

    /// 见 [`stored_file_path`]（播放端与面板显示端共用那一套解析）。
    fn resolve_stored_file(&self) -> Option<PathBuf> {
        stored_file_path(&self.file)
    }

    fn fault_once(&mut self, msg: &str) {
        if !self.fault_logged {
            self.fault_logged = true;
            log::warn!("击键音效：{msg}（同因后续静默）");
        }
    }
}

/// 索引 → 文件名（`ctx.sounds` 里第 `idx` 个，1 起；越界/≤0 → 落回默认）。
fn pick_index(idx: i64, sounds: &[String]) -> Option<SoundPick> {
    if idx >= 1 {
        sounds.get((idx - 1) as usize).cloned().map(SoundPick::File)
    } else {
        None
    }
}

impl Default for KeySoundModule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Modifiers;

    fn mods(ctrl: bool, alt: bool, shift: bool, win: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            win,
        }
    }

    /// ★ 音量出厂值统一（2026-09-26）：击键音效与点击音效同为 **100**，且模块 `new()`
    /// 不许自带字面量——抄一份就会出现"面板显示 100、实际按 80 播"的影子默认值。
    #[test]
    fn default_volume_matches_click_sound_and_module_initial() {
        let cfg = KeySoundSettings::default();
        assert_eq!(
            cfg.volume as u32,
            crate::core::config::TrailSettings::default().sound_volume,
            "击键音效与点击音效的出厂音量必须同值"
        );
        assert_eq!(cfg.volume, 100, "出厂音量 = 100");
        assert_eq!(
            KeySoundModule::new().volume,
            cfg.volume,
            "模块初值必须取自配置默认值（禁止另抄字面量）"
        );
    }

    /// ★ 面板「音效文件」行显示的就是 [`stored_file_path`] 的结果：必须**是绝对路径**
    /// （显示完整路径），且那条路径真的存在（"显示的文件"="会响的文件"）。
    #[test]
    fn stored_file_path_is_absolute_and_points_at_a_real_file() {
        let p = stored_file_path("click3").expect("出厂 click3 应解析得到");
        assert!(p.is_absolute(), "面板要显示完整路径：{}", p.display());
        assert!(p.is_file(), "显示的那条路径必须真的存在：{}", p.display());
        assert!(p.ends_with("click3.wav"));
        assert!(stored_file_path("").is_none());
        assert!(stored_file_path("  ").is_none());
    }

    /// 压缩出一个独立音效文件夹（含给定文件名），返回 (目录, 文件名列表)。
    fn make_sound_dir(files: &[&str]) -> (std::path::PathBuf, Vec<String>) {
        let dir = std::env::temp_dir().join(format!(
            "curseen_ks_snd_{}_{}",
            std::process::id(),
            files.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"RIFF____WAVE").unwrap();
        }
        let names = wav_filenames(&dir.to_string_lossy());
        (dir, names)
    }

    #[test]
    fn wav_filenames_lists_supported_audio_sorted() {
        let (dir, names) = make_sound_dir(&["b.wav", "a.wav", "ignore.txt", "c.mp3"]);
        assert_eq!(
            names,
            vec!["a.wav", "b.wav", "c.mp3"],
            "只收可播格式且按名排序"
        );
        // 未配置目录 → 空（脚本落回默认音）。
        assert!(wav_filenames("").is_empty());
        // 目录不存在 → 空（静默）。
        assert!(wav_filenames("Z:\\\\no_such_dir_xyz").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 统一音效：返回 nil 落回默认音（click3）；长按连发出声=false 时返回 false 静音。
    #[test]
    fn uniform_script_falls_back_to_default_and_can_mute_repeats() {
        let mut m = KeySoundModule::new();
        m.sync(&KeySoundSettings {
            enabled: true,
            script: "KeySound/unified_sound".into(),
            volume: 50,
            file: "click3".into(),
            params: HashMap::new(),
        });
        // 平常返回 nil → 落回默认音（click3，真实能播）。
        assert!(
            m.on_key_down(KeyCode::Char('A'), mods(false, false, false, false), false),
            "字母键应落到默认音发声"
        );
        assert!(
            m.on_key_down(KeyCode::LCtrl, mods(true, false, false, false), false),
            "Ctrl 单独按下也要发声（全键覆盖）"
        );
        // 长按连发出声=false 且是长按重复 → 强制静音（连默认音也不响）。
        let mut p = HashMap::new();
        p.insert("repeat_sound".to_string(), serde_json::json!(false));
        let mut script_params = HashMap::new();
        script_params.insert("KeySound/unified_sound".to_string(), p);
        m.sync(&KeySoundSettings {
            enabled: true,
            script: "KeySound/unified_sound".into(),
            volume: 50,
            file: "click3".into(),
            params: script_params,
        });
        assert!(
            !m.on_key_down(KeyCode::Char('A'), mods(false, false, false, false), true),
            "长按连发出声=false 且长按重复应静音"
        );
    }

    /// 分类音效按五类映射到各自的 `file` 参数值（2026-09-25 起音源与目录解耦：
    /// 脚本只做决策返回文件名，文件缺失由宿主解析链回落默认音并记日志）。
    #[test]
    fn classified_script_maps_the_five_categories() {
        // params：五类各挑一个文件名（`file` 参数的值，不必真实存在）。
        let mut p = HashMap::new();
        p.insert("fn_sound".to_string(), serde_json::json!("fn.wav"));
        p.insert("mod_sound".to_string(), serde_json::json!("mod.wav"));
        p.insert("symbol_sound".to_string(), serde_json::json!("sym.wav"));
        p.insert("digit_sound".to_string(), serde_json::json!("dig.wav"));
        p.insert("letter_sound".to_string(), serde_json::json!("let.wav"));
        let mut script_params = HashMap::new();
        script_params.insert("KeySound/class_sound".to_string(), p);

        let mut m = KeySoundModule::new();
        m.sync(&KeySoundSettings {
            enabled: true,
            script: "KeySound/class_sound".into(),
            volume: 50,
            file: "click3".into(),
            params: script_params,
        });
        let mut want = |key: KeyCode| match m.run_script(key, Modifiers::default(), false) {
            Some(SoundPick::File(f)) => f,
            other => panic!("分类音效应给出文件名，实际 {other:?}"),
        };
        assert_eq!(want(KeyCode::F(5)), "fn.wav", "功能键");
        assert_eq!(want(KeyCode::F(1)), "fn.wav", "功能键下限");
        assert_eq!(want(KeyCode::LCtrl), "mod.wav", "修饰键（左）");
        assert_eq!(want(KeyCode::RAlt), "mod.wav", "修饰键（右）");
        assert_eq!(want(KeyCode::Char(';')), "sym.wav", "符号键");
        assert_eq!(want(KeyCode::Char('[')), "sym.wav", "符号键");
        assert_eq!(want(KeyCode::Space), "sym.wav", "其余键归符号桶");
        assert_eq!(want(KeyCode::Char('5')), "dig.wav", "数字键");
        assert_eq!(want(KeyCode::Char('0')), "dig.wav", "数字键下限");
        assert_eq!(want(KeyCode::Char('A')), "let.wav", "字母键");
        assert_eq!(want(KeyCode::Char('z')), "let.wav", "字母键（小写归一）");
    }

    /// 未挑音效（params 空）：分类音效挑不出文件 → 返回 nil → 落回默认音。
    #[test]
    fn classified_without_sound_dir_falls_back_to_default() {
        let mut m = KeySoundModule::new();
        m.sync(&KeySoundSettings {
            enabled: true,
            script: "KeySound/class_sound".into(),
            volume: 50,
            file: "click3".into(),
            params: HashMap::new(),
        });
        assert!(
            m.run_script(KeyCode::Char('A'), Modifiers::default(), false)
                .is_none(),
            "未挑音效时分类音效应落回默认音"
        );
    }

    /// 返回 `false` → 强制静音；返回索引 → 取 `ctx.sounds` 里对应项。
    #[test]
    fn index_returns_pick_silent_for_false() {
        let dir = std::env::temp_dir().join(format!("curseen_ks_idx_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("idx.lua"),
            "function run(ctx, params)\n  if params.mute then return false end\n  return 2\nend\n",
        )
        .unwrap();
        let mut m = KeySoundModule::new();
        m.script_path = dir.join("idx.lua");
        m.sounds = vec!["a.wav".into(), "b.wav".into()];
        // 返回 2 → 取第二个文件（b.wav）。
        assert_eq!(
            match m.run_script(KeyCode::Char('A'), Modifiers::default(), false) {
                Some(SoundPick::File(f)) => f,
                other => panic!("应取到索引 2 的文件，实际 {other:?}"),
            },
            "b.wav"
        );
        // mute=true → 返回 false → 强制静音。
        let mut p = HashMap::new();
        p.insert("mute".to_string(), serde_json::json!(true));
        m.params = p;
        assert!(
            matches!(
                m.run_script(KeyCode::Char('A'), Modifiers::default(), false),
                Some(SoundPick::Silent)
            ),
            "mute 应返回强制静音"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏脚本（没有 run）：静默 + 不 panic。
    #[test]
    fn script_without_run_is_silent_not_panicky() {
        let dir = std::env::temp_dir().join(format!("curseen_ks_bad_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("bad.lua"), "-- 什么都没有").unwrap();
        let mut m = KeySoundModule::new();
        m.script_path = dir.join("bad.lua");
        assert!(!m.on_key_down(KeyCode::Char('A'), mods(false, false, false, false), false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 统一 Lua 注册表：`role="keysound"` 枚举到两个预设脚本，schema 符合 2026-09-25
    /// 新契约（音效选择全是 `file` 浏览参数；无 folder/sound 参数）。
    #[test]
    fn lua_registry_finds_keysound_presets_with_new_schemas() {
        let mut reg =
            crate::scripting::registry_ext::LuaExtRegistry::new(&crate::core::paths::lua_ext_dir());
        reg.scan();
        let ks = reg.role_of("keysound");
        let ids: Vec<&str> = ks.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"KeySound/unified_sound"),
            "统一音效应在 keysound 列表：{ids:?}"
        );
        assert!(
            ids.contains(&"KeySound/class_sound"),
            "分类音效应在 keysound 列表：{ids:?}"
        );
        let u = reg.get("KeySound/unified_sound").expect("统一音效 schema");
        assert_eq!(
            u.params.len(),
            2,
            "统一音效 = 「音效」file 浏览 + 「长按连发出声」bool"
        );
        assert!(
            u.params
                .iter()
                .any(|p| p.key == "sound" && p.ptype == crate::scripting::ParamType::File),
            "统一音效的音源应是 file 浏览参数"
        );
        let c = reg.get("KeySound/class_sound").expect("分类音效 schema");
        assert!(
            !c.params.iter().any(|p| matches!(
                p.ptype,
                crate::scripting::ParamType::Folder | crate::scripting::ParamType::Sound
            )),
            "分类音效不再有 folder/sound 参数——五类音效全部改为 file 浏览"
        );
        assert_eq!(
            c.params
                .iter()
                .filter(|p| p.ptype == crate::scripting::ParamType::File)
                .count(),
            5,
            "五类各一个 file 浏览参数"
        );
        assert!(
            c.params.iter().any(|p| p.key == "letter_sound"),
            "五类键名不能变（老配置靠它对上）"
        );
    }
}
