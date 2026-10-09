//! 按键声音目录：全部来自 `assets/Sounds/` 下的音频文件。
//!
//! 与 [`super::effects::EffectCatalog`] 同构，但简单得多——声音没有 `schema()`，一个文件
//! 就是一个选项。
//!
//! 面板每次打开都会重建 [`SoundCatalog`]（即重扫目录），所以用户往目录里丢一个新的音频
//! 文件，下次开面板就能在五键的下拉里选到——**扩展不需要重编译**。
//!
//! # 显示名与 id 是两回事
//!
//! - **id**（配置里存的、喂给播放器的）= 文件名 stem，不含扩展名。这样用户把 `.wav`
//!   换成同名 `.mp3`（或反过来）不会让配置失效。
//! - **显示名** = `stem.ext`，带扩展名——**格式是用户需要看见的信息**，也是同名不同格式
//!   时区分两项的唯一办法。超长时按 [`crate::ui::card::DROPDOWN_TEXT_W`] 的像素预算截断
//!   stem，**保留扩展名**。
//!
//! # 列表里不会有"放不出声"的项
//!
//! 扫描只收 Windows 原生就能播的格式（见 `platform::audio::SUPPORTED_EXT`），系统没解码器
//! 的一律**不进列表**。所以这里不需要"可播性"标志，面板也不用为死选项写提示行——
//! 能出现在下拉里的，点下去就该有声音。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windui::prelude::*;

use super::card;
use super::tokens;

/// 声音目录（相对 exe 同级 `assets/Sounds`）。
pub use crate::core::paths::sounds_dir;

/// 声音目录。
#[derive(Clone)]
pub struct SoundCatalog {
    /// 配置 id（文件名 stem），与 `names` 同序。
    ids: Rc<RefCell<Vec<String>>>,
    /// 下拉显示名（`stem.ext`，按 `DROPDOWN_TEXT_W` 截断），绑到下拉控件。
    pub names: Signal<Vec<String>>,
    dir: PathBuf,
}

impl SoundCatalog {
    pub fn new() -> Self {
        Self::with_dir(sounds_dir())
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        let (ids, names) = scan(&dir);
        Self {
            ids: Rc::new(RefCell::new(ids)),
            names: signal(names),
            dir,
        }
    }

    /// 重扫目录并把新的名字列表推给信号。
    ///
    /// 预留给「不重建面板也要刷新列表」的场景（当前面板重建时本就会重扫）。
    pub fn refresh(&self) {
        let (ids, names) = scan(&self.dir);
        *self.ids.borrow_mut() = ids;
        self.names.set(names);
    }

    /// 当前下拉的显示名列表。
    pub fn display_names(&self) -> Vec<String> {
        self.names.get()
    }

    /// 选中下标 → 配置 id（文件名 stem）。越界时回退空串（= 不发声）。
    pub fn id_at(&self, idx: usize) -> String {
        self.ids.borrow().get(idx).cloned().unwrap_or_default()
    }

    /// 配置 id → 下标。未知名字回退 0（指向第一个可用声音，保证总选得中一个）。
    pub fn index_of(&self, id: &str) -> usize {
        self.ids
            .borrow()
            .iter()
            .position(|x| x == id)
            .unwrap_or_default()
    }
}

impl Default for SoundCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// 击键音效「音效目录」行 + 音效下拉的候选项（2026-09-21）。
///
/// # 为什么要这一层
///
/// 分类音效要给五类按键各配一个音，写死 `click1..click5` 就只能在这五个里挑。用户要的
/// 是"五类各选各的、音源随便挑"——于是音源交给一个**目录**：用户挑一次文件夹，五个下拉
/// 同时列出该目录里所有能播的音频。目录扫描必须落在宿主（Lua 没有 io）。
///
/// # 值的两种写法（解析端 `modules::key_sound::resolve_sound` 两种都认）
///
/// | 文件所在位置 | 落盘的值 | 理由 |
/// |---|---|---|
/// | 出厂 `Sounds` | **stem**（`click3`） | 与素材一起搬家不失效 |
/// | 用户挑的外部目录 | **绝对路径**（`D:\音效包\thock.wav`） | 不复制文件：整包拷贝进素材目录既慢又撑体积；原地引用即可 |
///
/// # 顺序与去重
///
/// 所选目录 → 出厂目录。出厂音**永远在列表里**（换目录后
/// 还回得去）；同名的只留靠前那一项（去重按 stem，避免两个同名项分不清）。
pub fn sound_choices(dir: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in candidate_dirs(dir) {
        for e in crate::platform::audio::list_audio(&d) {
            if !seen.insert(e.stem.clone()) {
                continue;
            }
            let value = if is_asset_dir(&d) {
                e.stem.clone()
            } else {
                // ★ 外部目录的音记绝对路径，但**先过便携化**：音效包若放在程序目录里
                //   （最能跟着搬家的一种放法），落盘的就是相对路径，换台电脑仍有效。
                crate::core::paths::to_app_relative(&d.join(format!("{}.{}", e.stem, e.ext)))
            };
            out.push((value, label_of(&e.stem, &e.ext)));
        }
    }
    out
}

/// 候选目录的扫描顺序：所选目录 → 素材目录 → 出厂目录。
///
/// `dir` 为空 = 还没挑过目录（出厂态）：只有后两级。
/// ★ 所选目录先过 [`from_app_relative`]：配置里存的是**便携写法**（相对程序目录，
/// 换台电脑也有效），扫描要的是真实路径。
fn candidate_dirs(dir: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let picked = dir.trim();
    if !picked.is_empty() {
        v.push(crate::core::paths::from_app_relative(picked));
    }
    v.push(sounds_dir());
    v
}

/// 这两个目录是不是同一个（素材目录解析到出厂目录时避免重复扫两遍）。
fn same_dir(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().trim_end_matches(['/', '\\'])
        == b.to_string_lossy().trim_end_matches(['/', '\\'])
}

/// 该目录是不是"跟着素材走"的目录（值记短名）。
fn is_asset_dir(d: &Path) -> bool {
    same_dir(d, &sounds_dir())
}

/// 「音效目录」行的显示文本：**恒为完整路径**。
///
/// 没挑过（出厂态）就显示**出厂音效目录的真实路径**——「出厂音效」是个标签不是目录，
/// 用户要的是"这个目录到底在哪"（2026-09-22 用户拍板；2026-09-25 起该目录改名为
/// `assets/Sounds` 并由各发声功能共用）。挑过就显示挑中的路径（配置里若是相对写法，这里补成绝对路径）。
/// 超宽由 `card::browse_row` 按 `PATH_TEXT_W` 收成 `…`。
pub fn sound_dir_display(dir: &str) -> String {
    let t = dir.trim();
    if t.is_empty() {
        return sounds_dir().to_string_lossy().into_owned();
    }
    crate::core::paths::from_app_relative(t)
        .to_string_lossy()
        .into_owned()
}

/// 「音效文件」行的显示文本：**完整路径**（不只是文件名）。
///
/// 走 [`crate::modules::key_sound::stored_file_path`]——与播放端同一套解析，所以面板上
/// 显示的这条路径**就是真正会响的那个文件**（出厂的 `click3` 落在 `assets/Sounds/`，
/// 短名在这里补成完整路径；浏览引用的外部文件原样显示绝对路径）。
/// 解析不到（文件被删了）时退化为显示原值，至少把用户填的东西原样还给他。
/// 值为空 → 空（浏览类行：要么空，要么给出选定对象）。
pub fn sound_file_display(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        return String::new();
    }
    crate::modules::key_sound::stored_file_path(n)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| n.to_string())
}

/// 扫目录，返回 (id 列表, 显示名列表)。两者同序。
fn scan(dir: &Path) -> (Vec<String>, Vec<String>) {
    let entries = crate::platform::audio::list_audio(dir);
    let ids = entries.iter().map(|e| e.stem.clone()).collect();
    let names = entries.iter().map(|e| label_of(&e.stem, &e.ext)).collect();
    (ids, names)
}

/// 下拉显示名：`stem.ext`，stem 超长时截断但**保留扩展名**（格式信息不能丢）。
fn label_of(stem: &str, ext: &str) -> String {
    let font = tokens::FONT_MD;
    let tail = format!(".{ext}");
    let budget = card::DROPDOWN_TEXT_W as f32 - card::text_width(&tail, font);
    format!("{}{tail}", fit(stem, budget.max(0.0), font))
}

/// 按像素预算截断字符串：放得下就原样，放不下就留前缀 + 省略号。
///
/// 宽度一律走 `card::char_width`（与 `text_width` 同一模型），否则"截完仍超预算"。
fn fit(s: &str, budget: f32, font: f32) -> String {
    if card::text_width(s, font) <= budget {
        return s.to_string();
    }
    let ellipsis_w = card::char_width('…', font);
    let mut out = String::new();
    let mut acc = 0.0f32;
    for c in s.chars() {
        let cw = card::char_width(c, font);
        if acc + cw + ellipsis_w > budget {
            break;
        }
        out.push(c);
        acc += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir_with(files: &[&str]) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("curseen_sfxcat_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"RIFF____WAVE").unwrap();
        }
        dir
    }

    /// 击键音效的候选表：素材目录里的音记**短名**、外部音效包里的音记**全路径**。
    #[test]
    fn sound_choices_keep_short_names_for_assets_and_abs_paths_for_packs() {
        let pack = temp_dir_with(&["thock.wav"]);
        let all = sound_choices(&pack.to_string_lossy());
        let (vals, labels): (Vec<_>, Vec<_>) = all.into_iter().unzip();
        let i = vals
            .iter()
            .position(|v| v.ends_with("thock.wav"))
            .expect("外部音效包里的音必须进列表");
        assert!(
            Path::new(&vals[i]).is_absolute(),
            "外部目录的音记全路径（不复制进素材）：{}",
            vals[i]
        );
        assert_eq!(labels[i], "thock.wav", "显示名 = stem.ext");
        // 出厂音永远在列表里 —— 换到自己的音效包之后还回得去。
        assert!(
            vals.iter().any(|v| v == "click1"),
            "出厂音记短名且始终可选：{vals:?}"
        );
        let _ = std::fs::remove_dir_all(&pack);
    }

    /// 没挑目录（出厂态）：值全是短名（跟着素材走）。
    #[test]
    fn sound_choices_without_a_dir_lists_only_asset_stems() {
        let all = sound_choices("");
        assert!(!all.is_empty(), "出厂态至少要列出出厂音效");
        assert!(
            all.iter()
                .all(|(v, _)| !v.contains('\\') && !v.contains('/')),
            "没挑目录时值全是短名：{all:?}"
        );
    }

    /// 目录行**恒显示完整路径**：没挑过 → 出厂目录的真实路径（不再是「出厂音效」这种
    /// 标签）；挑过 → 原样给回（不再只留最后一段）。
    #[test]
    fn sound_dir_display_is_always_a_full_path() {
        let factory = sounds_dir().to_string_lossy().into_owned();
        assert_eq!(sound_dir_display(""), factory);
        assert_eq!(sound_dir_display("   "), factory);
        assert_eq!(
            sound_dir_display(r"D:\音效包\机械键盘"),
            r"D:\音效包\机械键盘",
            "挑过的目录要显示完整路径，不能只剩最后一段目录名"
        );
    }

    /// 文件行显示**完整路径**：空值 → 空（浏览类行：要么空、要么给出选定对象）；
    /// 出厂短名 `click3` → 补成 Sounds 目录里的真实文件（不是"只有 click3"）。
    #[test]
    fn sound_file_display_shows_the_whole_path() {
        assert_eq!(sound_file_display(""), "");
        assert_eq!(sound_file_display("  "), "");
        let p = sound_file_display("click3");
        assert!(p.ends_with("click3.wav"), "应补出扩展名并给出完整路径：{p}");
        assert!(
            p.contains("Sounds"),
            "出厂音落在鼠标点击音效目录，路径里必须看得到：{p}"
        );
    }

    /// ★ 便携端到端（2026-09-22 用户拍板：默认路径要"换台电脑也有效"）。
    ///
    /// 判据链三段，任一段断了用户都会看到坏结果：
    /// 1. 程序目录里的音效包 → 候选表里记的是**相对路径**（否则换盘符/用户名就失效，
    ///    表现就是"换了电脑音效全没了"）；
    /// 2. 目录行把它**还原成绝对路径**显示（阶段 3 的"浏览类行恒显完整路径"不许被
    ///    相对化改回退——面板上出现 `assets/…` 这种相对路径同样是坏结果）；
    /// 3. 扫描时 `from_app_relative` 又能找回真实目录（候选表不为空）。
    #[test]
    fn a_pack_inside_the_app_dir_is_stored_portably_and_shown_absolutely() {
        // 程序目录下开一个临时音效包。用 `app_dir()` 而不是 `temp_dir()`：要测的正是
        // "在程序目录内"，而 `%TEMP%` 在程序目录外（那一路记绝对路径，另有测试覆盖）。
        let pack = crate::core::paths::app_dir().join("__sfx_portable_probe__");
        let _ = std::fs::remove_dir_all(&pack);
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("thock.wav"), b"RIFF____WAVE").unwrap();

        let all = sound_choices(&pack.to_string_lossy());
        let stored_file = all
            .iter()
            .map(|(v, _)| v)
            .find(|v| v.ends_with("thock.wav"))
            .cloned()
            .expect("音效包里的音必须进候选表");
        assert!(
            !Path::new(&stored_file).is_absolute(),
            "程序目录内的音效应当落成便携的相对路径：{stored_file}"
        );

        // 目录行的值 = 同一套便携写法；显示必须回到绝对路径。
        let stored_dir = crate::core::paths::to_app_relative(&pack);
        assert!(
            !Path::new(&stored_dir).is_absolute(),
            "程序目录内的音效目录应当落成相对路径：{stored_dir}"
        );
        assert_eq!(
            sound_dir_display(&stored_dir),
            pack.to_string_lossy(),
            "存的可以是相对路径，显示的必须是完整路径"
        );
        assert!(
            Path::new(&sound_dir_display(&stored_dir)).is_absolute(),
            "目录行显示的必须是绝对路径"
        );

        // 换台电脑的模拟：`from_app_relative` 以**当前**程序目录为基准拼回真实路径。
        assert_eq!(crate::core::paths::from_app_relative(&stored_dir), pack);
        assert_eq!(candidate_dirs(&stored_dir).first(), Some(&pack));
        assert_eq!(
            crate::modules::key_sound::stored_file_path(&stored_file),
            Some(pack.join("thock.wav")),
            "文件行/播放端也要能按同一套便携写法找回真实文件"
        );

        let _ = std::fs::remove_dir_all(&pack);
    }

    #[test]
    fn ids_are_stems_and_labels_carry_the_extension() {
        let dir = temp_dir_with(&["zeta.wav", "alpha.mp3"]);
        let cat = SoundCatalog::with_dir(dir.clone());
        // id = stem（配置里存的就是它），显示名 = stem.ext
        assert_eq!(
            cat.display_names(),
            vec!["alpha.mp3".to_string(), "zeta.wav".to_string()]
        );
        assert_eq!(cat.id_at(0), "alpha");
        assert_eq!(cat.id_at(1), "zeta");
        // 配置里写 stem，也能解析回下标
        assert_eq!(cat.index_of("alpha"), 0);
        assert_eq!(cat.index_of("zeta"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_names_are_truncated_but_keep_the_extension() {
        let dir = temp_dir_with(&["foley_body_impact-01.wav"]);
        let cat = SoundCatalog::with_dir(dir.clone());
        let shown = &cat.display_names()[0];
        assert!(
            shown.ends_with(".wav"),
            "截断必须保留扩展名（格式是用户要看的）：{shown}"
        );
        assert!(shown.contains('…'), "超长名字应被截断：{shown}");
        // 截断后仍要在下拉文本预算内，否则会在按钮里折行
        let w = card::text_width(shown, tokens::FONT_MD);
        assert!(
            w <= card::DROPDOWN_TEXT_W as f32 + 1.0,
            "显示名 {shown} 宽 {w}，超出下拉文本预算 {}",
            card::DROPDOWN_TEXT_W
        );
        // 但 id 仍是完整 stem：配置与文件都能对上
        assert_eq!(cat.id_at(0), "foley_body_impact-01");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normal_names_are_not_touched() {
        // 常见短名不该被截断（避免"本来好好的名字硬加省略号"）
        assert_eq!(label_of("click1", "wav"), "click1.wav");
        assert_eq!(label_of("mouseclick1", "wav"), "mouseclick1.wav");
    }

    /// 系统解不了的格式**不进下拉**——列表里没有死选项，面板也就不需要"无解码器"提示行。
    #[test]
    fn undecodable_formats_never_reach_the_dropdown() {
        let dir = temp_dir_with(&["ok.wav", "foley_body_impact-01.wv"]);
        let cat = SoundCatalog::with_dir(dir.clone());
        assert_eq!(
            cat.display_names(),
            vec!["ok.wav".to_string()],
            "只有可播的 wav 进下拉；.wv 连扫都不扫"
        );
        assert_eq!(cat.index_of("foley_body_impact-01"), 0, "未知名字回退首项");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_of_falls_back_to_first() {
        let dir = temp_dir_with(&["only.wav"]);
        let cat = SoundCatalog::with_dir(dir.clone());
        assert_eq!(cat.index_of("only"), 0);
        assert_eq!(cat.index_of("被删掉的声音"), 0);
        assert_eq!(cat.id_at(99), "", "越界回退空串（不发声）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_dir_yields_empty_catalog() {
        let dir = std::env::temp_dir().join("curseen_sfxcat_empty_definitely_missing");
        let _ = std::fs::remove_dir_all(&dir);
        let cat = SoundCatalog::with_dir(dir);
        assert!(cat.display_names().is_empty());
        assert_eq!(cat.id_at(0), "");
        assert_eq!(cat.index_of("任意"), 0, "空目录下也不 panic");
    }
}
