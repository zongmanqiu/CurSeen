//! 拖尾效果目录：全部来自 `assets/MouseTrail/` 下的 Lua 脚本。
//!
//! 2.0 起所有内置拖尾特效都已改写为 Lua 脚本（见 `main/assets/MouseTrail/`），所以这里
//! 不再有"内置注册表 + 用户脚本"两套来源——下拉里只有 Lua 脚本，id 即文件名（裸 stem）。
//!
//! 面板每次打开面板都会重建 [`EffectCatalog`]（即重扫目录），所以新放进目录的脚本下次开
//! 面板就能选到。每个脚本通过 `schema()` 自报参数表，[`EffectCatalog::schema_for`] 把它
//! 取出来喂给主窗口动态生成参数 UI。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windui::prelude::*;

use crate::scripting::{LuaEffectRegistry, ParamDef};

/// 用户脚本目录（相对 exe 同级 `assets/MouseTrail`）。
pub use crate::core::paths::mousetrail_dir;

/// 一次扫描的结果：id 列表 / 显示名列表 / schema 表（三者同序或同键）。
type Scanned = (Vec<String>, Vec<String>, HashMap<String, Vec<ParamDef>>);

/// 效果目录。
#[derive(Clone)]
pub struct EffectCatalog {
    /// 与 `names` 同序的配置 id（文件名 stem，`config.trail.trail_effect` 里写的就是它）。
    ids: Rc<RefCell<Vec<String>>>,
    /// 显示名，绑定下拉控件。
    pub names: Signal<Vec<String>>,
    /// 每个 id 的参数声明（供动态 UI）。
    schemas: Rc<RefCell<HashMap<String, Vec<ParamDef>>>>,
    /// 重扫闭包：目录来源可能不止一个（见 [`hoverdot_catalog`]），扫法各自不同，
    /// 把"怎么扫"封在闭包里，[`EffectCatalog::refresh`] 与构造共用同一份逻辑。
    rescan: Rc<dyn Fn() -> Scanned>,
}

impl EffectCatalog {
    pub fn new() -> Self {
        Self::with_dir(mousetrail_dir())
    }

    pub fn with_dir(scripts_dir: PathBuf) -> Self {
        Self::from_rescan(Rc::new(move || scan(&scripts_dir)))
    }

    /// 从重扫闭包构造（多源目录的统一入口）。
    fn from_rescan(rescan: Rc<dyn Fn() -> Scanned>) -> Self {
        let (ids, names, schemas) = rescan();
        Self {
            ids: Rc::new(RefCell::new(ids)),
            names: signal(names),
            schemas: Rc::new(RefCell::new(schemas)),
            rescan,
        }
    }

    /// 悬浮标脚本目录（`assets/HoverDot/`，id = 裸 stem）。
    ///
    /// ★ 2026-09-26 用户口径定稿：`scripts/` 里的通用脚本**不做**悬浮标下拉的统一
    /// 收录——用户要复合效果（拖尾 + 点击声音 + …）走复合脚本宿主（`role="composite"`，
    /// 脚本页运行开关），而不是把特效脚本塞进下拉冒充悬浮标。
    pub fn hoverdot() -> Self {
        Self::with_dir(crate::core::paths::hoverdot_dir())
    }

    /// 重扫脚本目录并把新的显示名 / schema 推给信号。
    ///
    /// 预留给「不重建下拉也要刷新列表」的场景（当前面板重建时本就会重扫）。
    pub fn refresh(&self) {
        let (ids, names, schemas) = (self.rescan)();
        *self.ids.borrow_mut() = ids;
        *self.schemas.borrow_mut() = schemas;
        self.names.set(names);
    }

    /// 当前下拉的显示名列表。
    pub fn display_names(&self) -> Vec<String> {
        self.names.get()
    }

    /// 显示名下标 → 配置 id。越界时回退到第一个（保证永远写得出合法 id）。
    pub fn id_at(&self, idx: usize) -> String {
        self.ids.borrow().get(idx).cloned().unwrap_or_default()
    }

    /// 配置 id → 显示名下标。未知 id 回退 0。
    ///
    /// 兼容 2.0.0 之前写 `lua:xxx` 的旧配置：先按原样找，再去掉 `lua:` 前缀找，最后加上
    /// 前缀找，都失败才回退 0。当前所有 id 都是裸 stem，但保留容错避免老配置丢特效。
    pub fn index_of(&self, id: &str) -> usize {
        let ids = self.ids.borrow();
        if let Some(i) = ids.iter().position(|x| x == id) {
            return i;
        }
        let stripped = id.strip_prefix("lua:").unwrap_or(id);
        if let Some(i) = ids.iter().position(|x| x == stripped) {
            return i;
        }
        if let Some(i) = ids
            .iter()
            .position(|x| x.as_str() == format!("lua:{stripped}"))
        {
            return i;
        }
        0
    }

    /// 配置里指定的 id 不在目录里 ⇒ **补进末尾**（显示名 `<id>（脚本缺失）`），返回下标。
    ///
    /// 与录屏音频设备、虚拟键盘方案同一条口径：**不许把用户配置静默改成第一项**。
    /// 面板会把这个"缺失"项原样写回配置；用户想换效果时自己从下拉里选。
    pub fn index_of_or_keep(&self, id: &str) -> usize {
        let stripped = id.strip_prefix("lua:").unwrap_or(id);
        let existing = {
            let ids = self.ids.borrow();
            ids.iter()
                .position(|x| x == id)
                .or_else(|| ids.iter().position(|x| x == stripped))
                .or_else(|| {
                    ids.iter()
                        .position(|x| x.as_str() == format!("lua:{stripped}"))
                })
        };
        if let Some(i) = existing {
            return i;
        }
        let mut ids = self.ids.borrow_mut();
        ids.push(id.to_string());
        let idx = ids.len() - 1;
        drop(ids);
        let mut names = self.names.get();
        names.push(format!("{id}（脚本缺失）"));
        self.names.set(names);
        self.schemas.borrow_mut().entry(id.to_string()).or_default();
        idx
    }

    /// 取某特效的参数声明（按 id 文件名）。不存在返回空。
    pub fn schema_for(&self, id: &str) -> Vec<ParamDef> {
        self.schemas.borrow().get(id).cloned().unwrap_or_default()
    }
}

impl Default for EffectCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// 下拉显示名的字符上限。
///
/// 下拉文本区约 158 逻辑像素（见 `ui::card::DROPDOWN_TEXT_W`，按控件组总宽 200
/// 推导）≈ 11 个汉字，超了会在按钮里折行。
const MAX_DISPLAY_CHARS: usize = 9;

/// 按显示预算截断：超长就留前 `MAX_DISPLAY_CHARS - 1` 个字符再加省略号。
fn shorten(name: &str) -> String {
    if name.chars().count() <= MAX_DISPLAY_CHARS {
        return name.to_string();
    }
    let mut out: String = name.chars().take(MAX_DISPLAY_CHARS - 1).collect();
    out.push('…');
    out
}

/// 扫 `assets/MouseTrail/`，返回 (id 列表, 显示名列表, schema 表)。顺序按 id 字典序。
fn scan(scripts_dir: &Path) -> Scanned {
    let mut reg = LuaEffectRegistry::new(scripts_dir);
    reg.scan();
    finish_scan(reg.list().into_iter().cloned().collect())
}

/// 扫描结果 → (ids, names, schemas) 三元组（names 过 display 预算）。
fn finish_scan(list: Vec<crate::scripting::EffectSchema>) -> Scanned {
    let ids: Vec<String> = list.iter().map(|s| s.id.clone()).collect();
    let names: Vec<String> = list.iter().map(|s| shorten(&s.name)).collect();
    let schemas: HashMap<String, Vec<ParamDef>> = list
        .iter()
        .map(|s| (s.id.clone(), s.params.clone()))
        .collect();
    (ids, names, schemas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir_with(scripts: &[(&str, &str)]) -> (std::path::PathBuf, EffectCatalog) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // 目录名带自增序号：并行测试线程若派生同名目录（如两个测试都以 "glow" 开头），
        // 先跑的那个会被后跑的 remove_dir_all 掏空，扫描结果变成空集（曾致
        // schema_for_parses_params 与 index_of 测试互相踩踏、随机失败）。
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "curseen_cat_{}_{}_{}",
            std::process::id(),
            n,
            scripts[0].0
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in scripts {
            std::fs::write(dir.join(format!("{name}.lua")), body).unwrap();
        }
        let cat = EffectCatalog::with_dir(dir.clone());
        (dir, cat)
    }

    #[test]
    fn empty_dir_yields_empty_catalog() {
        let dir = std::env::temp_dir().join("curseen_cat_empty");
        let _ = std::fs::remove_dir_all(&dir);
        let cat = EffectCatalog::with_dir(dir);
        assert_eq!(cat.display_names().len(), 0);
        assert_eq!(cat.id_at(0), "");
    }

    #[test]
    fn lua_scripts_sorted_by_id() {
        let (_d, cat) = temp_dir_with(&[
            ("zeta", "effect_name = \"Z 效果\"\n"),
            ("alpha", "effect_name = \"A 效果\"\n"),
        ]);
        assert_eq!(
            cat.display_names(),
            vec!["A 效果".to_string(), "Z 效果".to_string()]
        );
        assert_eq!(cat.id_at(0), "alpha");
        assert_eq!(cat.id_at(1), "zeta");
    }

    #[test]
    fn index_of_and_id_at_roundtrip_with_legacy_prefix() {
        let (_d, cat) = temp_dir_with(&[("glow", "effect_name = \"辉光\"\n")]);
        let idx = cat.index_of("glow");
        assert_eq!(cat.id_at(idx), "glow");
        // 老配置可能写 "lua:glow"
        let idx2 = cat.index_of("lua:glow");
        assert_eq!(cat.id_at(idx2), "glow");
        // 未知 id 回退 0
        assert_eq!(cat.index_of("不存在"), 0);
    }

    #[test]
    fn schema_for_parses_params() {
        let body = r##"
effect_name = "辉光圆影"
function schema()
  return {
    { key="density", label="密度", type="int", min=1, max=50, step=1, default=5 },
    { key="color_mode", label="颜色模式", type="choice", choices={"rainbow","fixed"}, default="rainbow" },
    { key="fixed_color", label="固定色", type="color", default="#FFFF00" },
  }
end
"##;
        let (_d, cat) = temp_dir_with(&[("glow", body)]);
        let schema = cat.schema_for("glow");
        assert_eq!(schema.len(), 3);
        assert_eq!(schema[0].key, "density");
        assert_eq!(schema[0].ptype, crate::scripting::ParamType::Int);
        assert_eq!(
            schema[1].choices,
            vec!["rainbow".to_string(), "fixed".to_string()]
        );
        assert_eq!(schema[2].default.as_str().unwrap_or(""), "#FFFF00");
    }

    #[test]
    fn overlong_display_names_are_shortened() {
        assert_eq!(shorten("测试粒子"), "测试粒子");
        assert_eq!(shorten("一二三四五六七八九十"), "一二三四五六七八…");
        assert_eq!(shorten("abc"), "abc");
    }

    /// 配置里的 id 不在目录里 ⇒ 补进末尾并原样写回，**不许静默改成第一项**。
    #[test]
    fn keeps_a_configured_effect_missing_from_the_directory() {
        let dir = std::env::temp_dir().join(format!(
            "curseen_fx_keep_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cat = EffectCatalog::with_dir(dir.clone());
        assert!(cat.display_names().is_empty());
        let i = cat.index_of_or_keep("waves");
        assert_eq!(i, 0);
        assert_eq!(cat.id_at(i), "waves", "写回的必须是配置原值");
        assert_eq!(
            cat.display_names(),
            vec!["waves（脚本缺失）".to_string()],
            "下拉里要明确告诉用户脚本没了"
        );
        assert_eq!(cat.index_of_or_keep("waves"), 0, "不许重复追加");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
