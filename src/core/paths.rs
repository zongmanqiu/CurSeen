//! 随程序分发的资源路径，以及"便携化"的数据落点。
//!
//! `assets/MouseTrail/` 这类目录**不能**只按当前工作目录找：从资源管理器双击、从任务栏启动、
//! 被别的程序拉起（自启动、更新器重启）时 CWD 各不相同，相对路径会时灵时不灵——表现就是
//! "同一份程序，有时候能选到 Lua 效果，有时候选项里一个都没有"。
//!
//! 规则：以**可执行文件所在目录**为基准；那里没有该目录时回退到 CWD 下的同名路径，
//! 这样 `cargo run`（CWD = crate 根）与安装后（CWD 未必是安装目录）都能工作。
//!
//! ★ 便携化（用户需求 `新建 文本文档.txt`）：程序解压后直接运行，配置、日志、更新中转
//! 文件**都留在程序目录**，不往 `%APPDATA%` 写；这样"卸载"就是删目录。所以
//! [`data_dir`] 就是 exe 同级，`config.json` / `log/` / `update/` 都挂在它下面——
//! 根目录里只有文件夹、exe 和配置文件。
//!
//! ★ 配置**只认** [`config_path`] 这一处：启动时 exe 同级有 `config.json` 就读，没有
//! 就用出厂默认（首次落盘时生成）。`%APPDATA%`（1.0 的地盘）**不读也不写**——那边
//! 写了什么与本程序无关。

use std::path::{Path, PathBuf};

/// 程序目录：可执行文件所在目录（拿不到 exe 路径时回落到当前工作目录）。
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 便携数据目录 = 程序目录。
///
/// 单独开一个函数而不是到处写 [`app_dir`]：以后若要改成 `data/` 子目录或允许用户指定，
/// 只有这一处要动。
pub fn data_dir() -> PathBuf {
    app_dir()
}

/// 单个脚本的私有数据目录：`<程序目录>/data/lua/<脚本 id>/`（按需创建）。
///
/// 与 `assets/` **分离**：脚本写自己的 JSON/缓存不会触发目录监听/面板重扫；与 `temp/`
/// 分离：不是一次性产物，不会被临时清理。用户设置仍归 `config.json` 的
/// `lua_ext.params[id]`，这里只放脚本自己的状态/缓存。
///
/// id 的路径段做最小净化（只留 ASCII 字母数字与 `. _ -`，丢弃空/`.`/`..`），既保留
/// `scripts/2.ocr_text` 这种可读层级，又不让脚本名把目录带出 `data/lua`。
pub fn script_data_dir(id: &str) -> PathBuf {
    let mut dir = data_dir().join("data").join("lua");
    for part in id.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        let safe: String = part
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if !safe.is_empty() {
            dir.push(safe);
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 配置文件：`<程序目录>/config.json`。
pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

/// 日志目录：`<程序目录>/log`。
///
/// 用户要求"和 curseen.exe 同级目录增加一个 log 文件夹"——升级/启动期的排查全靠它。
pub fn log_dir() -> PathBuf {
    data_dir().join("log")
}

/// 更新中转目录：`<程序目录>/update`。
///
/// 下载的 zip 与替换用的 bat 都落这里，替换完成后由 bat 自行清掉。放在子目录而不是直接
/// 摊在根目录，是为了守住"根目录只有文件夹、exe、配置文件"的版图约定。
pub fn update_dir() -> PathBuf {
    data_dir().join("update")
}

/// 临时产物根目录：`<程序目录>/temp`。
///
/// ★ 版图约定（用户要求，2026-09-17）：**程序自己产生的、用户没有明确指定位置的产物**
/// 统一落在这里，按功能模块分子目录（见 [`module_temp_dir`]）。这条约定替代了原来那套
/// "用户填一个保存目录、空则退回 `图片/CurSeen`"的做法——面板上不再有「保存目录」控件，
/// 用户要留下的东西走各自的「另存为」。
///
/// 与 [`log_dir`] / [`update_dir`] 并列，都挂在程序目录下，保持便携：删目录 = 卸载干净。
pub fn temp_dir() -> PathBuf {
    data_dir().join("temp")
}

/// 截图产物的模块目录名：`temp/screenshot`。
pub const MODULE_SCREENSHOT: &str = "screenshot";

/// MP4 录屏产物的模块目录名：`temp/record`。
pub const MODULE_RECORD: &str = "record";

/// GIF 动图产物的模块目录名：`temp/gif`。
///
/// ★ 这个常量被**两处**共用，别把它当成"随手起的目录名"：
///   1. 录完落盘 —— `runtime::Runtime::start_recording_now` 的 GIF 分支；
///   2. 裁剪窗「打开…」的初始目录 —— `runtime::layer::gif_trim::TrimCanvas::pick_file`。
///
///   写死两个字面量 `"gif"` 时改一处就会出现"录到 A、打开默认在 B"，而且这种错**不会
///   报**任何东西，只是每次都要重新点目录。共用常量是唯一能让它编译期就对上的做法。
pub const MODULE_GIF: &str = "gif";

/// 屏幕标注 PNG 产物的模块目录名：`temp/annotation`。
///
/// ★ 2026-09-23 迁移：旧落点是 exe 同级的「标注」文件夹（`app_dir()/"标注"`）——它是
///   程序目录里**唯一**一个"程序自己产生的产物目录"，既破坏了"根目录只有文件夹、exe 和
///   配置文件"的版图约定，也让"运行时产物统一在 `temp/` 下"这条规律出现例外。用户口径：
///   "我们所有运行过程中特地保存的文件都要放在这个文件夹里"。
pub const MODULE_ANNOTATION: &str = "annotation";

/// 某功能模块的临时产物目录：`<程序目录>/temp/<module>`（**按需创建**）。
///
/// 一个模块一个子目录，与 `assets/<功能>` 同一个版图思路：新增模块不必改解析逻辑，
/// 只要给个模块名。现役模块名见 [`MODULE_SCREENSHOT`] / [`MODULE_RECORD`] / [`MODULE_GIF`]。
///
/// ★ `gif` 与 `record` 是**两个目录而不是一个**（用户口径 2026-09-20）：两者产物
///   格式、体积、用途都不同，而 GIF 还要被裁剪窗当"打开时的默认目录"用 —— 混在
///   `record/` 里就得先滤掉一堆 `.mp4`。目录名就是筛选条件。
///
/// 建不出来时**退回临时根目录**（再退程序目录）：绝不因为取不到目录而让功能失败——
/// 截图/录屏的产物位置是"尽力而为"的，功能本身不该被目录问题挡住。
pub fn module_temp_dir(module: &str) -> PathBuf {
    let dir = temp_dir().join(module);
    if std::fs::create_dir_all(&dir).is_ok() {
        return dir;
    }
    let root = temp_dir();
    if std::fs::create_dir_all(&root).is_ok() {
        return root;
    }
    data_dir()
}

/// 功能资源目录的通用解析器：`<exe 目录>/assets/<功能>`（回退 `<CWD>/...`）。
///
/// ★ 版图约定（用户要求）：**一个功能一个文件夹**，功能自己的脚本、声音、图片都放在
/// 它那一个文件夹下（`assets/README.md` 是版图说明）。新增功能不必再改这里的解析逻辑，
/// 只要给个功能名即可。
///
/// 名字写错是静默失败（目录不存在 → 空列表），所以各个功能都走 [`asset_dir`] 的下层
/// 具名包装（[`mousetrail_dir`] / [`hoverdot_dir`] / [`sounds_dir`] /
/// [`clickfx_dir`]），让拼错也拼不到别处去。
/// ★ 2026-09-25 大扫除：`keysound_dir`（中转目录，浏览即引用后无存在必要）与
/// `about_dir`（关于页从未接入 Logo/二维码，零消费方）已删除。
pub fn asset_dir(feature: &str) -> PathBuf {
    resolve(Path::new("assets").join(feature))
}

/// 击键回显的键名映射表：`assets/KeyDisplay/keynames.ini`。
///
/// 用户改这个文件就能改每个键位在弹窗里显示的文字（保存后按一次键即重读，
/// 无需重启）。格式与键名清单见文件头注释；缺失的键回退内置默认名。
pub fn keynames_ini_path() -> PathBuf {
    asset_dir("KeyDisplay").join("keynames.ini")
}

/// 用户 Lua 鼠标拖尾效果脚本目录：`assets/lua/MouseTrail`。
///
/// 统一 Lua 框架下拖尾 effect 脚本收敛到 `assets/lua/` 的命名子目录：
/// 所有内置示例特效与用户自写脚本都放这里——既是给用户看的"怎么写拖尾"范例，
/// 也是程序读取脚本的入口（见 `modules::trail`；下拉 id = 子目录内文件裸 stem）。
pub fn mousetrail_dir() -> PathBuf {
    lua_ext_dir().join("MouseTrail")
}

/// 悬浮标素材目录：`assets/lua/HoverDot`。
///
/// 「PNG 图片」脚本（`png.lua`）从这里取图；用户在面板里选的本地 PNG 也会被**复制**
/// 进来（配置只记文件名，素材与配置一起搬家不会失效）。其余类型的脚本（ring/solid/…）
/// 也在同一子目录。
pub fn hoverdot_dir() -> PathBuf {
    lua_ext_dir().join("HoverDot")
}

/// 声音素材目录：`assets/Sounds`（**共用**，2026-09-25 由 `ClickSound` 改名）。
///
/// ★ 所有发声功能（鼠标点击声音、键盘声音、拖尾音效）的出厂音源都默认指向它；
/// 用户也可以在各自面板里浏览选别的文件夹（浏览即引用，程序目录内的记便携相对路径）。
/// 运行期唯一来源：面板下拉 = 扫描本目录的音频文件名，播放时按名读盘
/// （见 `platform::audio`）。用户丢一个新 `.wav` 进来就能在面板选到，**不需要重编译**
/// —— 与 `MouseTrail` / `HoverDot` / `ClickFx` 同一套扩展约定。
pub fn sounds_dir() -> PathBuf {
    asset_dir("Sounds")
}

/// 点击效果自定义脚本目录：`assets/lua/ClickFx`。
///
/// 统一 Lua 框架下点击效果 effect 脚本（用户自写的点按特效，每次按下触发一次）收敛到
/// `assets/lua/` 的命名子目录，与 `MouseTrail`/`HoverDot` 同构；面板扫描子目录列出
/// 可选脚本，参数 UI 由脚本 `schema()` 驱动。
pub fn clickfx_dir() -> PathBuf {
    lua_ext_dir().join("ClickFx")
}

/// Lua 扩展脚本目录：`assets/lua`。
///
/// 图片右键菜单的「脚本」子菜单和 UI 的脚本页都扫这个目录。每个脚本定义
/// `name` / `description` / `menu` / `schema()` 并通过全局 `ext` 调用本程序开放的 API。
pub fn lua_ext_dir() -> PathBuf {
    // 单元测试跑 `tests/fixtures/assets/lua`：出厂鼠标包已从 `main/assets` 清出，
    // 但引擎/模型门禁仍要对着完整脚本树跑。集成测试与生产走 `asset_dir`。
    #[cfg(test)]
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
    #[cfg(not(test))]
    let dir = asset_dir("lua");
    dir
}

/// 屏幕标注的 PNG 导出目录：`<程序目录>/temp/annotation`。
///
/// 与其他运行时产物同构（见 [`module_temp_dir`] 与 [`MODULE_ANNOTATION`]）：程序自己
/// 产生的、用户没有明确指定位置的东西，一律落在 exe 同级的 `temp/` 下按模块分子目录。
pub fn annotations_dir() -> PathBuf {
    module_temp_dir(MODULE_ANNOTATION)
}

/// 落盘用的**便携写法**：能相对程序目录表示的就写成相对路径，否则如实记绝对路径。
///
/// 为什么：配置跟着程序走（便携版——删目录即卸载），而绝对路径里写着盘符与用户目录，
/// 换台电脑（或换个盘符/用户名）就整条失效——表现是"换了电脑音效全没了"。所以凡是
/// 让用户挑的路径（音效目录、导入失败的音效文件）落盘前都过这一道。
///
/// ★ 只有在**程序目录内**才相对化：跨盘符没法相对（Windows 没有 `..` 跨盘的路径），
/// 程序目录外的文件本来也带不走，如实记绝对路径比编一个假相对路径诚实。
pub fn to_app_relative(p: &Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    match abs.strip_prefix(app_dir()) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => p.to_string_lossy().into_owned(),
    }
}

/// 便携写法的逆运算：相对路径按**程序目录**拼成绝对路径；绝对路径原样。
///
/// 与 [`to_app_relative`] 成对出现——落盘前相对化、读回来先绝对化，于是"配置里存的是
/// 什么"和"面板上显示/实际播放用的是什么"解耦：存便携、用绝对。
pub fn from_app_relative(s: &str) -> PathBuf {
    let t = s.trim();
    if t.is_empty() {
        return PathBuf::new();
    }
    let p = Path::new(t);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    app_dir().join(p)
}

/// 把悬浮标图片的配置值解析成实际路径。
///
/// 配置里存的是**文件名**（导入到 `assets/lua/HoverDot/` 的情况）；老配置或导入失败时可能
/// 是绝对路径。规则：绝对路径且文件存在 → 原样用；否则按 `assets/lua/HoverDot/<name>` 拼。
pub fn hover_image_path(value: &str) -> PathBuf {
    let p = Path::new(value);
    if p.is_absolute() && p.is_file() {
        return p.to_path_buf();
    }
    hoverdot_dir().join(p)
}

/// 把相对路径解析成"exe 同级优先、CWD 兜底"的绝对/相对路径。
fn resolve(rel: PathBuf) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&rel);
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    rel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_relative_when_exe_dir_has_none() {
        // 测试进程的可执行文件在 target/debug/deps 下，同级不会有 assets/lua/MouseTrail，
        // 因此解析结果应当退回相对路径（而不是凭空造出一个不存在的绝对路径）。
        let p = mousetrail_dir();
        assert!(
            p.ends_with(Path::new("assets").join("lua").join("MouseTrail")),
            "解析结果应指向 assets/lua/MouseTrail，实际 {p:?}"
        );
        assert!(!p.is_absolute() || p.is_dir(), "绝对路径必须真实存在");
    }

    /// 版图护栏：每个功能文件夹都必须挂在 `assets/` 下（effect 进 `assets/lua/<子目录>`）。
    /// 名字拼错是静默失败（目录不存在 → 选项空），所以这里逐个钉死。
    #[test]
    fn every_feature_folder_lives_under_assets() {
        for (dir, name) in [
            (mousetrail_dir(), "MouseTrail"),
            (hoverdot_dir(), "HoverDot"),
            (sounds_dir(), "Sounds"),
            (clickfx_dir(), "ClickFx"),
        ] {
            assert!(
                dir.ends_with(Path::new("assets").join("lua").join(name))
                    || dir.ends_with(Path::new("assets").join(name)),
                "{name} 的解析结果错位：{dir:?}"
            );
        }
    }

    /// 悬浮标图片配置值：裸文件名 → `assets/lua/HoverDot` 下；绝对路径 → 原样用。
    #[test]
    fn hover_image_path_resolves_bare_name_into_the_feature_folder() {
        let p = hover_image_path("cursor.png");
        assert!(p.ends_with("cursor.png"), "文件名不该丢：{p:?}");
        assert!(
            p.parent().is_some_and(|d| d.ends_with("HoverDot")),
            "裸名应落到 HoverDot 文件夹：{p:?}"
        );
    }

    #[test]
    fn script_data_lives_outside_assets_and_sanitizes_id_segments() {
        let dir = script_data_dir("scripts/2.ocr_text");
        assert!(
            dir.ends_with(
                Path::new("data")
                    .join("lua")
                    .join("scripts")
                    .join("2.ocr_text")
            ),
            "{dir:?}"
        );
        assert!(
            !dir.to_string_lossy().contains("assets"),
            "不能落进 assets：{dir:?}"
        );
        assert!(dir.is_dir(), "取用时必须已建好：{dir:?}");
        let escaped = script_data_dir("../../evil/name");
        assert!(
            escaped.ends_with(Path::new("data").join("lua").join("evil").join("name")),
            "`..` 段应被丢弃：{escaped:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(escaped);
    }

    #[test]
    fn portable_paths_all_live_under_the_exe_dir() {
        let root = data_dir();
        assert_eq!(config_path(), root.join("config.json"));
        assert_eq!(log_dir(), root.join("log"));
        assert_eq!(update_dir(), root.join("update"));
        assert_eq!(temp_dir(), root.join("temp"));
        // 便携化的要害：一个字节都不该落到 %APPDATA% 去。
        assert_eq!(app_dir(), root);
        assert!(!root.to_string_lossy().contains("AppData"));
    }

    /// 临时产物目录的版图：`temp/<模块>`，且**每个模块一个子目录**（不摊在 temp 根下）。
    #[test]
    fn module_temp_dirs_are_namespaced_under_temp() {
        for name in [
            MODULE_SCREENSHOT,
            MODULE_RECORD,
            MODULE_GIF,
            MODULE_ANNOTATION,
        ] {
            let p = module_temp_dir(name);
            assert!(
                p.ends_with(Path::new("temp").join(name)),
                "{name} 的临时目录错位：{p:?}"
            );
        }
        // 不同模块不会互相覆盖。
        assert_ne!(
            module_temp_dir(MODULE_SCREENSHOT),
            module_temp_dir(MODULE_RECORD)
        );
        // ★ GIF 与 MP4 **必须**分开：裁剪窗把 `temp/gif` 当默认打开目录，两者混住就得先
        //   滤掉一堆 `.mp4`。这条断言守的是"别为了省事把 gif 名字改回 record"。
        assert_ne!(module_temp_dir(MODULE_GIF), module_temp_dir(MODULE_RECORD));
    }

    /// ★ 用户口径（2026-09-23）：**所有"运行过程中特地保存的文件"都在 `<exe>/temp` 下**，
    ///   一个模块一个子目录，屏幕标注的 PNG 也不例外。
    ///
    /// 这条判据的价值在于"堵例外"：旧实现里 `annotations_dir()` 是唯一一处直接
    /// `app_dir().join(中文名)` 的产物目录，功能上能用、观感上却把 `temp/` 这条规律捅了
    /// 一个洞——后来的人只会照它抄出第二个例外。断言"是 temp 的**直接**子目录"，就是为了
    /// 让下一个新增产物也无处可逃。
    #[test]
    fn every_runtime_artifact_lives_under_temp() {
        let temp = temp_dir();
        assert_eq!(annotations_dir(), temp.join(MODULE_ANNOTATION));
        for p in [
            annotations_dir(),
            module_temp_dir(MODULE_SCREENSHOT),
            module_temp_dir(MODULE_RECORD),
            module_temp_dir(MODULE_GIF),
        ] {
            assert_eq!(
                p.parent(),
                Some(temp.as_path()),
                "运行时产物应当是 temp 的直接子目录：{p:?}"
            );
        }
    }

    /// 模块目录名不能改：它已经落到用户的磁盘上了（`temp/record/*.mp4` 这些）。
    /// 改名会让老产物"凭空消失"（还在盘上，只是程序再也不往那儿看），而用户只会觉得
    /// 是程序把东西弄丢了。要改就必须配一次迁移。
    #[test]
    fn module_dir_names_are_frozen() {
        assert_eq!(MODULE_SCREENSHOT, "screenshot");
        assert_eq!(MODULE_RECORD, "record");
        assert_eq!(MODULE_GIF, "gif");
        assert_eq!(MODULE_ANNOTATION, "annotation");
        // 全是纯 ASCII 小写：中文/大写会让一部分压缩与同步工具出岔子，也没必要。
        for n in [
            MODULE_SCREENSHOT,
            MODULE_RECORD,
            MODULE_GIF,
            MODULE_ANNOTATION,
        ] {
            assert!(
                n.chars().all(|c| c.is_ascii_lowercase()),
                "模块目录名 {n} 不是纯小写 ASCII"
            );
        }
    }

    // ── 便携路径：落盘相对化 / 读回绝对化（2026-09-22 击键音效默认路径改造）──

    /// ★ 程序目录**内**的路径必须能来回一趟不丢字符。
    ///
    /// 判据刻意取非平凡样本（多级子目录 + **中文目录名** + 扩展名）：默认值往返是假绿，
    /// 只测 ASCII 短名则漏掉"中文被 `to_string_lossy` 弄坏"这一类。
    #[test]
    fn app_relative_round_trips_inside_the_app_dir() {
        let inside = app_dir()
            .join("assets")
            .join("Sounds")
            .join("机械键盘")
            .join("click3.wav");
        let stored = to_app_relative(&inside);
        assert!(
            !Path::new(&stored).is_absolute(),
            "程序目录内应当存成相对路径：{stored}"
        );
        assert_eq!(
            PathBuf::from(&stored),
            Path::new("assets")
                .join("Sounds")
                .join("机械键盘")
                .join("click3.wav"),
            "相对写法应当以程序目录为基准剥掉前缀"
        );
        assert_eq!(from_app_relative(&stored), inside, "相对写法必须能原样还原");
    }

    /// ★ 程序目录**外**的路径如实保留绝对路径，绝不编造相对路径。
    ///
    /// Windows 没有跨盘 `..`（`E:\` 相对不到 `D:\`），硬编出来的相对路径换台电脑
    /// 就指向莫名其妙的地方 —— 所以宁可"不可携带但诚实"。
    #[test]
    fn absolute_paths_outside_the_app_dir_are_kept_verbatim() {
        let outside = PathBuf::from(r"D:\音效包\机械键盘");
        let stored = to_app_relative(&outside);
        assert_eq!(
            PathBuf::from(&stored),
            outside,
            "程序目录外应如实记绝对路径，实际 {stored}"
        );
        assert_eq!(from_app_relative(&stored), outside);
    }

    /// 空值（出厂态）读回来是空路径 —— 「音效目录」靠这一位判"没挑过"。
    #[test]
    fn empty_value_reads_back_as_an_empty_path() {
        assert_eq!(from_app_relative(""), PathBuf::new());
        assert_eq!(from_app_relative("   "), PathBuf::new());
    }
}
