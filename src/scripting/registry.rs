//! Lua 效果注册表 + 脚本 Manifest（schema）解析。
//!
//! 扫描 `assets/MouseTrail/` 下的每个 `.lua`，用一次性 VM 跑出：
//! - `effect_name`（显示名）
//! - `schema()` 返回的参数表（名称/范围/默认值/类型）
//!
//! 解析结果就是 [`EffectSchema`]，既喂给主窗口动态生成参数 UI，也喂给控制器把参数
//! 注入 Lua。对标 MouseEffects 的 `GetConfigurationSchema()`：脚本声明"我要哪些参数、
//! 每个什么类型、范围多少、默认多少"，宿主据此自动造 UI、自动取值。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

use super::engine::ScriptingEngine;

/// 参数类型。直接决定主窗口用哪种控件（滑块 / 复选框 / 下拉 / 颜色 / 图片 /
/// 音效下拉 / 目录与文件浏览 / 文本框）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// 浮点滑块（0..1 映射到 [min,max]）。
    Float,
    /// 整数滑块（四舍五入后取 [min,max] 整数）。
    Int,
    /// 开关。
    Bool,
    /// 颜色选择器（hex 字符串 `#RRGGBB` / `#RRGGBBAA`）。
    Color,
    /// 下拉，从 `choices` 里选一个字符串值。
    ///
    /// ★ 与 [`Self::Sound`] 的差别只在**候选项由谁给**：本档的候选项是脚本 `choices` 写死的，
    ///   `Sound` 的候选项由宿主按当前音效目录扫描。两档的**值语义相同**（都是一个字符串），
    ///   所以宿主兜底时走的是同一个空串分支（见 `default_value`）——UI 那边显示的"首项"是
    ///   `choices` 的第一项，那是下拉的显示规则，不是这一档的默认值。
    Choice,
    /// ★ 音效下拉（击键音效「分类音效」用，2026-09-21）。
    ///
    /// 与 `Choice` 的唯一区别：**候选项不由脚本写死**，而由宿主按「音效目录」参数当前
    /// 指向的目录扫描（`ui::sfx::sound_choices`）——脚本没法做目录扫描（Lua 无 io），
    /// 而把 click1-5 写死在脚本里正是"五类只能选这五个音"的根源。值语义见
    /// `ui::sfx::sound_choices`：素材目录里的记短名（stem），外部目录里的记绝对路径。
    Sound,
    /// 文件夹选择（2026-09-21 引入，原击键音效「音效目录」行所用）。
    ///
    /// 值 = 目录的绝对路径；空串 = 出厂音效目录。UI 是「浏览…」按钮 + 目录名，
    /// 改了它，同一脚本里的 `Sound` 参数整批重建（下拉内容换一批）。
    /// ★ 2026-09-25 击键音效改版（「音效目录」行已删）后，内置脚本不再使用本类型，
    /// 仅为用户自定义脚本的兼容而保留。
    Folder,
    /// 图片文件接口（悬浮标复用同一套脚本的执行机制，支持用户导入自定义图片）。
    ///
    /// ★ 本期只落地"契约 + UI 占位"：脚本能声明它需要一个图片参数，UI 显示一个路径框，
    /// 但 `draw` 的实际图片加载/绘制留给下一期接入悬浮标时实现。
    Image,
    /// ★ 自由文本（2026-09-23，Lua 扩展脚本用）。
    ///
    /// UI 是一个可编辑文本框，值就是输入的原样字符串（`api_key` / 模型名 / 目录路径 /
    /// 任意参数都行）。落盘与喂给脚本的都是这个字符串本身。
    Str,
    /// ★ 音频文件浏览（2026-09-25，击键音效改版；同日二次改版：浏览即引用）。
    ///
    /// UI 是「浏览…」按钮：选中的文件**直接记路径**——程序目录内记便携相对路径
    /// （换台电脑仍有效），外部文件记绝对路径；不复制、无中转目录。脚本 `run`
    /// 返回这个路径，宿主按 `stored_file_path` 同一条解析链（绝对 / 便携相对 /
    /// Sounds 出厂兜底）找到真实文件播放。与 `Sound`（下拉）的区别：候选不由
    /// 目录扫描决定，用户直接挑文件——音效包里几十个音也只挑一个，下拉翻页反而难用。
    File,
    /// ★ 快捷键捕获框（2026-09-29）。
    ///
    /// UI 与屏幕标注的快捷键同一套：点击进入捕获态、按下的组合键写回框内。
    /// 值 = 规范化的按键串（`Ctrl+Alt+H`），落盘在 `params.<key>`。
    /// ★ 派发语义只对 `scripts/` 独立脚本生效（`Runtime::script_hotkeys` 只认
    ///   `scripts/` 前缀）；特效/贴图脚本声明它只是"存一个键位串"，不会被当成
    ///   全局热键——避免拖尾特效跟截图抢同一把键这种事故。
    Hotkey,
}

/// 单个参数声明（脚本 `schema()` 里的一行）。
#[derive(Debug, Clone)]
pub struct ParamDef {
    /// 配置键（脚本里 `params.<key>` 取用的名字）。
    pub key: String,
    /// UI 显示名。
    pub label: String,
    pub ptype: ParamType,
    /// 滑块 / 数值范围（bool/choice/sound/folder/str/file 忽略）。
    pub min: f32,
    pub max: f32,
    /// 滑块步进（仅 UI 用）。
    pub step: f32,
    /// 默认值（number / string / bool，随类型而定）。
    pub default: Json,
    /// choice 类型的候选项（字符串）。
    ///
    /// 这是**存进配置、喂给脚本 `params`** 的值，所以它必须是稳定的标识
    /// （`rainbow` / `fixed`），不能因为界面要显示中文就去改它——改了就等于让老配置里的
    /// 值全部失配。要显示别的字，用 [`Self::choice_labels`]。
    pub choices: Vec<String>,
    /// choice 各选项的**界面显示名**（可选，与 `choices` 一一对应）。
    ///
    /// 为空表示直接用 `choices` 本身当显示名（老脚本的默认行为）。
    /// 长度与 `choices` 不一致时整份忽略——半对半错的对照表比没有更糟。
    pub choice_labels: Vec<String>,
    /// 分组名（UI 可做分组标题，可选）。
    pub group: String,
    /// ★ 颜色参数**是否带透明度**（仅 `Color` 用，**默认 true**）。
    ///
    /// 2026-10-26 用户反馈：颜色选择器有的带透明度、有的不带，是严重的不一致。
    /// 现在 `type="color"` 默认就出透明度条、落盘走 8 位 `#RRGGBBAA`、脚本端
    /// `hex_to_rgb` 第四返回值就是 0–255 alpha；只有明确写 `alpha = false` 才退回
    /// 6 位 `#RRGGBB`（用于确实不允许半透明的颜色）。
    /// 两套透明度并存会互相打架，故必须二选一。
    pub alpha: bool,
}

impl ParamDef {
    /// 取默认值，按类型规整成可落盘的 JSON 值。
    pub fn default_value(&self) -> Json {
        if self.default.is_null() {
            // 没写 default 时给个类型安全的兜底
            match self.ptype {
                ParamType::Bool => Json::Bool(false),
                ParamType::Color => Json::String("#FF0000".into()),
                ParamType::Choice => {
                    Json::String(self.choices.first().cloned().unwrap_or_default())
                }
                // 音效/文件夹没写 default 时给空串：音效的"默认"由宿主的扫描结果决定
                // （首项），文件夹的空串语义就是"出厂目录"——不能拿 choices 去猜。
                // ★ `Hotkey` / `Image` 过去会掉进下面的数值兜底（交回 `1.0`），而界面显示的是
                //   空串——对拍格（`ui/model.rs::the_ui_initial_value_is_the_default_the_script_gets`）
                //   第一跑就是在这儿翻红的。这两档的值本来就是串：热键是键位串，图片是路径串。
                ParamType::Sound
                | ParamType::Folder
                | ParamType::Str
                | ParamType::File
                | ParamType::Hotkey
                | ParamType::Image => Json::String(String::new()),
                ParamType::Int => Json::Number((self.min as i64).into()),
                _ => {
                    Json::Number(serde_json::Number::from_f64(self.min as f64).unwrap_or(0.into()))
                }
            }
        } else {
            self.default.clone()
        }
    }

    /// 配置里存的值**符合这一格的声明类型**就原样交，否则（含"根本没这个键"）交
    /// [`Self::default_value()`]。
    ///
    /// ★ 这条的存在理由只有一个：**默认值必须有唯一一个来源**。界面控件过去各自
    ///   `raw.and_then(as_bool).unwrap_or_else(|| p.default…)`（`ui/model.rs::cell_from_param_in`），
    ///   脚本侧则什么都拿不到（`Runtime::lua_ext_params` 原样交配置）——同一个 schema
    ///   两种答案。现在两边都走这一个函数，等值由 `ui/model.rs` 里那格对拍钉住。
    ///
    /// 刻意**不**校验 `Choice` 的值在不在 `choices` 里：脚本改了候选项名字之后，老配置里那个
    /// 值该由脚本自己决定怎么办（回落首项是脚本的语义，不是宿主的），宿主只负责类型。
    pub fn value_or_default(&self, stored: Option<&Json>) -> Json {
        let fits = match stored {
            None => false,
            Some(v) => match self.ptype {
                ParamType::Bool => v.is_boolean(),
                ParamType::Int => v.is_i64(),
                ParamType::Choice
                | ParamType::Color
                | ParamType::Sound
                | ParamType::Folder
                | ParamType::Str
                | ParamType::File
                | ParamType::Hotkey => v.is_string(),
                // Float（以及未知 type 一律按数值收，见 `parse_schema_table` 的兜底档）
                _ => v.is_number(),
            },
        };
        match stored {
            Some(v) if fits => v.clone(),
            _ => self.default_value(),
        }
    }
}

/// 一个脚本的完整 Manifest：id（=文件名）+ 显示名 + 参数表。
#[derive(Debug, Clone)]
pub struct EffectSchema {
    /// 脚本 id（= 不带后缀的文件名），也是配置里存的值、拼 `<id>.lua` 路径的依据。
    pub id: String,
    /// 显示名（`effect_name`）。
    pub name: String,
    /// 参数声明列表（顺序即 UI 顺序）。
    pub params: Vec<ParamDef>,
}

/// Lua 效果注册表。
pub struct LuaEffectRegistry {
    scripts_dir: PathBuf,
    effects: HashMap<String, EffectSchema>,
}

impl LuaEffectRegistry {
    pub fn new(scripts_dir: &Path) -> Self {
        Self {
            scripts_dir: scripts_dir.to_path_buf(),
            effects: HashMap::new(),
        }
    }

    /// 扫描目录（递归），给每个 `.lua` 跑一个临时 VM 取出 name + schema。
    ///
    /// ★ 2026-10-26：id 从“文件名 stem”改为“相对 `scripts_dir` 的路径”。顶层脚本
    ///   id 仍是裸 stem（老配置零迁移）；子文件夹脚本得到 `CursorFX/fire` 这样的 id，
    ///   于是“官方目录下再分来源文件夹”不再被扫描器静默丢掉。
    pub fn scan(&mut self) {
        self.effects.clear();
        let mut pending = vec![self.scripts_dir.clone()];
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                    files.push(path);
                }
            }
        }
        files.sort();
        for path in files {
            let rel = path
                .strip_prefix(&self.scripts_dir)
                .unwrap_or(&path)
                .with_extension("");
            let id = rel.to_string_lossy().replace('\\', "/");
            if id.is_empty() {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let schema = match ScriptingEngine::new(&self.scripts_dir)
                .and_then(|e| e.load_script(&id).map(|_| e))
            {
                Ok(engine) => {
                    // ★ 鼠标三目录拒绝非鼠标角色（2026-09-28 用户口径）：
                    //   贴图右键菜单脚本（`role = "image"`）或键音脚本（`role = "keysound"`）
                    //   被丢进 `MouseTrail/` 时，不许出现在鼠标下拉里——它们各有归宿
                    //   （贴图右键子菜单 / 键盘页）。此前只看"是不是 .lua 文件"，于是下拉
                    //   里会躺一个选中后毫无效果的条目，比压根看不见更糟。
                    //   ★ 未声明 role 的照收：在这三个目录里 role 缺省就是 effect，
                    //   **归属由目录决定**，用户随手写的特效不该被扫描器静默丢弃。
                    let role: String = engine.lua().globals().get("role").unwrap_or_default();
                    if matches!(role.as_str(), "image" | "keysound") {
                        log::warn!(
                            "扫描 Lua 效果 {id}：role = {role} 属于非鼠标宿主，\
                             已从鼠标下拉移除（归宿：贴图右键菜单 / 键盘页）"
                        );
                        continue;
                    }
                    // ★ 统一契约：`name` 优先、`effect_name` 兼容；同一个脚本复制到
                    //   `scripts/` 或三个鼠标目录，标题都不会退回文件名。
                    let name = engine
                        .lua()
                        .globals()
                        .get::<Option<String>>("name")
                        .ok()
                        .flatten()
                        .or_else(|| engine.get_effect_name().ok())
                        .unwrap_or_else(|| id.clone());
                    let params = match engine.call_schema() {
                        Ok(tbl) => parse_schema_table(&tbl),
                        Err(_) => Vec::new(),
                    };
                    EffectSchema {
                        id: id.clone(),
                        name,
                        params,
                    }
                }
                Err(e) => {
                    log::warn!("扫描 Lua 效果 {id} 失败: {e}");
                    EffectSchema {
                        id: id.clone(),
                        name: stem.clone(),
                        params: vec![],
                    }
                }
            };
            self.effects.insert(id, schema);
        }
    }

    /// 按 id（文件名）取 schema。
    pub fn get_schema(&self, id: &str) -> Option<&EffectSchema> {
        self.effects.get(id)
    }

    /// 所有已注册的 schema，按 id 字典序排列（保证下拉顺序稳定）。
    pub fn list(&self) -> Vec<&EffectSchema> {
        let mut v: Vec<&EffectSchema> = self.effects.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn scripts_dir(&self) -> &Path {
        &self.scripts_dir
    }
}

/// 把 Lua `schema()` 返回的表解析成 `Vec<ParamDef>`。
pub fn parse_schema_table(tbl: &mlua::Table) -> Vec<ParamDef> {
    let mut out = Vec::new();
    for i in 1..=tbl.raw_len() {
        let Ok(entry) = tbl.get::<mlua::Table>(i) else {
            continue;
        };
        let key: String = entry.get("key").unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        let label: String = entry.get("label").unwrap_or_else(|_| key.clone());
        let ptype = match entry.get::<String>("type").unwrap_or_default().as_str() {
            "int" => ParamType::Int,
            "bool" => ParamType::Bool,
            "color" => ParamType::Color,
            "choice" => ParamType::Choice,
            "sound" => ParamType::Sound,
            "folder" => ParamType::Folder,
            "image" => ParamType::Image,
            "str" => ParamType::Str,
            "file" => ParamType::File,
            "hotkey" => ParamType::Hotkey,
            _ => ParamType::Float,
        };
        let min: f32 = entry.get("min").unwrap_or(0.0);
        let max: f32 = entry.get("max").unwrap_or(1.0);
        let step: f32 = entry.get("step").unwrap_or(0.01);
        let default = lua_value_to_json(entry.get("default").ok());
        let choices: Vec<String> = match entry.get::<mlua::Table>("choices") {
            Ok(c) => {
                let mut v = Vec::new();
                for j in 1..=c.raw_len() {
                    if let Ok(s) = c.get::<String>(j) {
                        v.push(s);
                    }
                }
                v
            }
            Err(_) => Vec::new(),
        };
        let group: String = entry.get("group").unwrap_or_default();
        // 颜色默认带透明度；脚本可显式 `alpha = false` 关掉（非 color 参数写了也无效）。
        // ★ 不能用 `get::<bool>().unwrap_or(true)`：mlua 会把缺失键的 Nil 转成 false，
        //   导致所有没写 alpha 的颜色被静默关掉透明度（用户反馈"颜色选择器没有透明度"）。
        let alpha: bool = match entry.get::<mlua::Value>("alpha") {
            Ok(mlua::Value::Boolean(b)) => b,
            _ => true,
        };
        // 可选的中文显示名（`labels = {"彩虹","单色"}`）。长度对不上就整份丢弃，
        // 只显示一半中文另一半英文比全英文更糟。
        let choice_labels: Vec<String> = match entry.get::<mlua::Table>("labels") {
            Ok(c) => {
                let mut v = Vec::new();
                for j in 1..=c.raw_len() {
                    if let Ok(s) = c.get::<String>(j) {
                        v.push(s);
                    }
                }
                if v.len() == choices.len() {
                    v
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        };
        out.push(ParamDef {
            key,
            label,
            ptype,
            min,
            max,
            step,
            default,
            choices,
            choice_labels,
            group,
            alpha,
        });
    }
    out
}

/// Lua value → JSON value（number / string / bool / integer）。
fn lua_value_to_json(v: Option<mlua::Value>) -> Json {
    match v {
        Some(mlua::Value::Number(n)) => Json::from(n),
        Some(mlua::Value::Integer(i)) => Json::from(i),
        Some(mlua::Value::String(s)) => Json::String(s.to_string_lossy()),
        Some(mlua::Value::Boolean(b)) => Json::Bool(b),
        _ => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一格声明：`default` 传 `Json::Null` 表示"脚本没写 default"（走类型兜底那一档）。
    fn def(ptype: ParamType, default: Json) -> ParamDef {
        ParamDef {
            key: "k".to_string(),
            label: "K".to_string(),
            ptype,
            min: 1.0,
            max: 9.0,
            step: 1.0,
            default,
            choices: vec!["a".to_string(), "b".to_string()],
            choice_labels: Vec::new(),
            group: String::new(),
            alpha: true,
        }
    }

    /// ★ `value_or_default` 的类型矩阵：**每一档声明类型**都要 ①缺键给默认、②类型对的原样、
    /// ③类型不对的回退。写成循环而不是挑三档，是因为"未知 type 一律按数值收"这一档
    /// （`parse_schema_table` 的兜底）最容易在只测 bool/int 时悄悄飘掉。
    #[test]
    fn value_or_default_follows_the_declared_type_for_every_kind() {
        let stringy = [
            ParamType::Choice,
            ParamType::Color,
            ParamType::Sound,
            ParamType::Folder,
            ParamType::Str,
            ParamType::File,
            ParamType::Hotkey,
        ];
        for t in stringy {
            let d = def(t, Json::String("keep".into()));
            assert_eq!(
                d.value_or_default(None),
                Json::String("keep".into()),
                "{t:?} 缺键该交默认值"
            );
            assert_eq!(
                d.value_or_default(Some(&Json::String("typed".into()))),
                Json::String("typed".into()),
                "{t:?} 类型对必须原样交"
            );
            assert_eq!(
                d.value_or_default(Some(&Json::Bool(true))),
                Json::String("keep".into()),
                "{t:?} 存的值是布尔 ⇒ 不算这档的类型"
            );
        }
        let b = def(ParamType::Bool, Json::Bool(true));
        assert_eq!(
            b.value_or_default(Some(&Json::Bool(false))),
            Json::Bool(false)
        );
        assert_eq!(
            b.value_or_default(Some(&Json::from(0_i64))),
            Json::Bool(true)
        );
        // Int 只认整数：带小数的存值在这儿就是"类型不符"，不许悄悄四舍五入。
        let i = def(ParamType::Int, Json::from(5_i64));
        assert_eq!(
            i.value_or_default(Some(&Json::from(3_i64))),
            Json::from(3_i64)
        );
        assert_eq!(i.value_or_default(None), Json::from(5_i64));
        assert_eq!(
            i.default_value(),
            Json::from(5_i64),
            "写了 default 就不走 min 兜底"
        );
        // 没写 default 的 Int 兜到 min（`default_value` 那条既有规则，本轮只引用不重写）。
        assert_eq!(
            def(ParamType::Int, Json::Null).value_or_default(None),
            Json::from(1_i64)
        );
        let f = def(ParamType::Float, Json::Null);
        assert_eq!(
            f.value_or_default(Some(&Json::from(2_i64))),
            Json::from(2_i64)
        );
        assert_eq!(
            f.value_or_default(None),
            Json::from(1.0_f64),
            "Float 缺 default 兜到 min"
        );
    }

    /// ★ `Choice` 刻意**不**校验"值在不在候选项里"：脚本改了候选项名字之后老配置该怎么回落
    /// 是脚本的语义，宿主只负责类型。这条测试是把那个决定钉住，不是随手一测。
    #[test]
    fn a_choice_value_outside_the_declared_list_is_passed_through_not_rejected() {
        let c = def(ParamType::Choice, Json::String("a".to_string()));
        assert_eq!(
            c.value_or_default(Some(&Json::String("gone_mode".into()))),
            Json::String("gone_mode".into()),
            "候选项变了不该由宿主替脚本决定回落成哪个"
        );
    }
}
