//! 离屏驱动：把**真实脚本**按真实宿主的顺序跑 N 帧，逐帧记账。
//!
//! 存在的理由：发行版里没有 cargo，也没有测试套件。外部 AI / CI 想判断"这个脚本对不对"，
//! 唯一能拿到的事实就是"跑一遍、看每一帧产出了什么"。所以这里刻意**不复刻**测试里的
//! 假脚本，而是用 `ScriptingEngine` 走与宿主同一条序列：
//!
//! ```text
//! 注环境 → on_move（真有移动才发）→ 每 1/120s 一个 step 子步 → draw
//! ```
//!
//! ★ 为什么不用 `FxCore`（四宿主共用的生产核心）：它把每一次 `call_*` 的错误都 `let _ =`
//!   吞掉了。对"跑着别崩"是对的，对"告诉我哪一行错了"是致命的——AI 迭代时最需要的就是
//!   那条带行号的 Lua 报错。这里每一步都 `?` 上来，错误带帧号。
//!
//! ★ 像素与坐标口径与真机一致：`screen_w/h` 是逻辑像素，`mouse_x/y` 与绘制坐标同一空间，
//!   `elapsed` 是**秒**（`FIXED_DT` 累加），`wall_ms` 是墙钟毫秒。任何一个换单位，
//!   "离屏通过、真机不对"就会重现（本项目历史上踩过 1000 倍的那次）。
//!
//! ★ 一帧 = 1/60 秒 = 两个 1/120 子步：与 `FxCore::advance(1/60)` 交付的子步数一致。

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value as Json};

use crate::core::draw::DrawList;
use crate::scripting::api_draw;
use crate::scripting::engine::ScriptingEngine;

/// 固定步长（秒）：与 `modules::trail::controller::FIXED_DT` 同一常量。
const DT: f32 = 1.0 / 120.0;
/// 一帧几个子步（1/60 s）。
const SUBSTEPS: u32 = 2;
/// 单帧指令数的硬上限：超过它基本等于脚本里写了 O(n²) 或死循环式生成。
pub const MAX_CMDS_PER_FRAME: usize = 20_000;
/// 采样点上限（给 AI 看的逐帧时间线，别把 300 帧全塞进 JSON）。
const TIMELINE_POINTS: usize = 24;

/// 输入结束后最多再跑多少帧来等"收敛"（20 秒）。
///
/// 为什么需要它：`settles` 判的是"**永不到 0**"（bug，静止态永久烧 CPU），不是"慢"。
/// 弹簧链的阻尼比只有约 0.16（欠阻尼），停止输入后还会真实地摆 6~9 秒——用"默认 300 帧
/// 内必须归零"去判，会把正确的脚本判红。所以这里给足预算，只在超预算还不停时才判死。
const SETTLE_BUDGET: u32 = 1200;

/// 脚本挂在哪个宿主上（决定 UI 入口与"怎么算收敛"）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    /// `HoverDot/`：悬浮标。静止必须**停止续帧**（`draw` 返回 nil）。
    Hover,
    /// `MouseTrail/`：拖尾。静止后可以合法地一直画（蛛丝缠网就是），故只提示不判死。
    Trail,
    /// `ClickFx/`：点击效果。实例必须在上限内自行停笔（否则永不释放）。
    Click,
    /// `scripts/`：复合脚本。与悬浮标同口径（它也吃 `draw` 返回值）。
    Composite,
    /// `KeySound/`：击键音效。契约是 `run(ctx, params)`（按键触发），**不是**逐帧绘制，
    /// 本工具驱动不了它——识别出来只是为了不说假话（别把它当成复合脚本报 host=panel）。
    KeySound,
}

impl Host {
    /// 目录名 → 宿主。目录即挂载点（见 `scripting::registry_ext` 的 role 兼容策略）。
    pub fn from_top_dir(top: &str) -> Option<Self> {
        match top {
            "HoverDot" => Some(Self::Hover),
            "MouseTrail" => Some(Self::Trail),
            "ClickFx" => Some(Self::Click),
            "scripts" => Some(Self::Composite),
            "KeySound" => Some(Self::KeySound),
            _ => None,
        }
    }

    /// 写进 Lua 全局 `host` 的值（与生产四宿主逐字一致）。
    pub fn host_global(self) -> &'static str {
        match self {
            Self::Hover => "hover",
            Self::Trail => "trail",
            Self::Click => "clickfx",
            Self::Composite => "panel",
            Self::KeySound => "keysound",
        }
    }

    /// 这个目录在面板上的入口（写给 AI 看：放错地方 = 用户找不到）。
    pub fn ui_entry(self) -> &'static str {
        match self {
            Self::Hover => "「悬浮标」卡片的类型下拉",
            Self::Trail => "「拖尾效果」卡片的效果下拉",
            Self::Click => "「点击效果」各键的下拉",
            Self::Composite => "「脚本」页的常驻卡片（不进上面三个下拉）",
            Self::KeySound => "「点击声音」卡片的音效脚本",
        }
    }

    /// 该目录是否支持热重载（改文件即时生效，无需重开面板/重启）。
    ///
    /// ★ 只有 `scripts/` 有（`Companion::ensure_fresh` 每帧 stat）。
    ///   三个特效目录的实例在 id 不变时**保住旧 VM**，改文件不生效——这是"AI 写完看不到
    ///   变化"的头号坑，所以必须作为事实输出，别让调用方猜。
    pub fn hot_reload(self) -> bool {
        matches!(self, Self::Composite)
    }

    /// 收敛判据用哪个信号（写进 JSON，让调用方知道该看哪一列）。
    fn settle_signal(self) -> &'static str {
        match self {
            Self::Hover | Self::Composite => "frame_request",
            Self::Trail | Self::Click => "produced",
            Self::KeySound => "n/a",
        }
    }
}

/// 合成输入的行为轮廓。
///
/// ★ 为什么不止一种：`HoverDot/locate`（晃动定位）平时**什么都不画**，只有"先静止够久、
///   再左右回摆"才弹环。只给一条"移动→停住"的轨迹，它必然一条指令都产不出，工具就会把
///   一个正确的脚本判成 fail。判据自己先错，比没有判据更糟。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// 横向 S 曲线扫过画面，然后停住（大多数跟随类 / 轨迹类脚本）。
    Sweep,
    /// 先静止 1.5 s，再左右各摆三次，然后停住（"静止触发"类脚本，如晃动定位）。
    IdleSwing,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sweep" => Some(Self::Sweep),
            "idle-swing" | "idle_swing" | "swing" => Some(Self::IdleSwing),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sweep => "sweep",
            Self::IdleSwing => "idle-swing",
        }
    }
}

/// 一帧的输入。
#[derive(Clone, Copy)]
struct Step {
    x: f32,
    y: f32,
    /// 这一帧是否要发 `on_move`。
    moving: bool,
}

/// 生成整段输入计划。固定（不用随机），所以同一个脚本每次跑出的账本一致。
fn plan(profile: Profile, frames: u32, w: f32, h: f32) -> Vec<Step> {
    let mut v: Vec<Step> = Vec::with_capacity(frames as usize);
    match profile {
        Profile::Sweep => {
            let moving = sweep_frames(frames);
            for i in 0..frames {
                let t = if moving == 0 {
                    0.0
                } else {
                    f32::min(i as f32, moving as f32) / moving as f32
                };
                let x = w * (0.18 + 0.64 * t);
                let y = h * (0.50 + 0.22 * (t * std::f32::consts::TAU * 1.5).sin());
                v.push(Step {
                    x,
                    y,
                    moving: i < moving,
                });
            }
        }
        Profile::IdleSwing => {
            let (idle, legs, leg) = idle_swing_band();
            let (cx, cy) = (w * 0.5, h * 0.5);
            // 摆幅取屏幕宽的 3%，夹在 [60, 120]：够越过默认阈值 40，又不至于甩出画面。
            let amp = (w * 0.03).clamp(60.0, 120.0);
            for i in 0..frames {
                let mut step = Step {
                    x: cx,
                    y: cy,
                    moving: false,
                };
                if i >= idle && i < idle + legs * leg {
                    let k = (i - idle) / leg;
                    let within = (i - idle) % leg;
                    let from = if k == 0 { 0.0 } else { leg_dir(k - 1) * amp };
                    let to = leg_dir(k) * amp;
                    let t = (within + 1) as f32 / leg as f32;
                    step.x = cx + from + (to - from) * t;
                    step.moving = true;
                }
                v.push(step);
            }
        }
    }
    v
}

/// 第 k 段的位移方向（+1 / -1 交替）。
fn leg_dir(k: u32) -> f32 {
    if k.is_multiple_of(2) {
        1.0
    } else {
        -1.0
    }
}

/// 静止 90 帧（1.5 s，越过 `locate` 默认 `idle_s = 1.0`）× 6 段 × 每段 4 帧。
fn idle_swing_band() -> (u32, u32, u32) {
    (90, 6, 4)
}

fn sweep_frames(frames: u32) -> u32 {
    (frames / 2).min(90)
}

/// 一帧的账。
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub cmds: usize,
    pub produced: bool,
    /// `draw` 返回的续帧毫秒数（只有悬浮标/复合宿主消费；其它宿主记下来仅供参考）。
    pub frame_ms: Option<u32>,
}

/// 一次离屏运行的完整账本。
#[derive(Debug, Clone)]
pub struct Report {
    pub host: Host,
    pub profile: Profile,
    /// 实际跑了多少帧（收敛后早停，可能少于请求值）。
    pub frames_total: u32,
    /// 请求跑了多少帧。
    pub frames_requested: u32,
    /// 输入区间 `[start, end)`：这段里每帧都发 `on_move`。
    pub band: (u32, u32),
    pub frames: Vec<Frame>,
    /// 脚本源码里是否用了 `ctx:image(...)`：这类脚本没配图就画不出东西，
    /// 首帧 / 产量判据必须跳过，否则会把"没导图"误报成"脚本坏了"。
    pub uses_image: bool,
    pub notes: Vec<String>,
}

impl Report {
    /// 逐帧账本 → 判据结论。
    ///
    /// 判据全部**由工具自己算**，不让调用方（AI）去解释数字：它只要看 `verdict`。
    /// 这一条是刻意的——"AI 觉得没问题"不是判据，`pass/fail` 才是。
    pub fn checks(&self) -> Checks {
        let mut failed: Vec<&'static str> = Vec::new();
        let mut notes = self.notes.clone();

        let first = self.frames.first().copied();
        let first_draws = match first {
            Some(_) if self.uses_image => None,
            Some(f) => Some(f.produced),
            None => Some(false),
        };
        // 静止触发的脚本（idle-swing 轮廓）首帧本来就该什么都不画，那条判据对它不适用。
        if first_draws == Some(false) && !self.uses_image && self.profile == Profile::Sweep {
            failed.push("first_frame_draws");
            notes.push(
                "第 0 帧没有任何绘制指令：宿主按「这一帧画不画」判活，首帧不画 = 实例当场判死。\
                 （常见原因：把淡入写成了 age=0 返回 nil）"
                    .into(),
            );
        }

        let (bs, be) = self.band;
        let moving_draws = if self.uses_image || be <= bs {
            None
        } else {
            Some(
                self.frames[bs as usize..(be as usize).min(self.frames.len())]
                    .iter()
                    .any(|f| f.produced),
            )
        };
        if moving_draws == Some(false) {
            failed.push("draws_while_moving");
            notes.push(
                "指针移动期间一帧都没画出来：脚本没有把 cursor_x/cursor_y 变成指令，\
                 或者所有图元都被透明度 / 尺寸判据挡掉了。"
                    .into(),
            );
        }

        let (settle_at, perpetual) = self.settle_point();
        if settle_at.is_none() && be < self.frames_total {
            match self.host {
                // 悬浮标 / 复合：静止后必须归零，否则"静止 0 功耗"这条地基就断了。
                Host::Hover | Host::Composite => {
                    failed.push("settles");
                    notes.push(
                        "指针停住后一直在要帧（draw 返回了正数却永不到 0）：这不是错，是代价——\
                         静止态会永久烧 CPU。请让收敛判定更宽容（残差永远不会精确等于 0）。"
                            .into(),
                    );
                }
                // 点击效果：实例不停止就等于永不释放（管理器按"还画不画"剔除）。
                Host::Click => {
                    failed.push("stops");
                    notes.push("按下后一直在画，没有停笔：实例永远不会被剔除。".into());
                }
                // 键音脚本根本不会被逐帧驱动（run 已提前返回错误），这里只是穷尽匹配。
                Host::KeySound => {}
                // 拖尾：蛛丝缠网这类本来就该一直在变，只提示。
                Host::Trail => {
                    notes.push(
                        "离开指针后仍在持续产出指令。对「蛛丝缠网」这类整体在变的特效是正常的；\
                         如果本来该停，就是老化判据没生效。"
                            .into(),
                    );
                }
            }
        }

        let peak = self.frames.iter().map(|f| f.cmds).max().unwrap_or(0);
        if peak > MAX_CMDS_PER_FRAME {
            failed.push("cmds_sane");
            notes.push(format!(
                "单帧指令数峰值 {peak} 超过 {MAX_CMDS_PER_FRAME}：几乎必然是生成循环写成了 O(n²) 或没设上限。"
            ));
        }

        let total_cmds: usize = self.frames.iter().map(|f| f.cmds).sum();
        if total_cmds == 0 {
            if self.uses_image {
                // ★ 图片类脚本（`HoverDot/png` 那样全靠 `ctx:image`）在离屏里**没有位图**，
                //   零指令是正确行为。这里若判 fail，工具会指着出厂脚本说"它坏了"，
                //   而 AI 会照着去改一个没坏的东西——判据自己先错比没有判据更糟。
                notes.push(format!(
                    "整段（{} 帧）零指令：脚本用的 `ctx:image` 在离屏里没有位图（要真机 + 已导入的\
                     图片），产量类判据一律跳过。`verdict=pass` 只代表「能加载、参数合法、不空转」，\
                     不代表画面对过——这一档必须靠真机验收。",
                    self.frames_total
                ));
            } else {
                failed.push("draws_at_all");
                notes.push(format!(
                    "整段（{} 帧，输入轮廓 {}）一条绘制指令都没有。",
                    self.frames_total,
                    self.profile.as_str()
                ));
            }
        }

        Checks {
            failed,
            first_draws,
            moving_draws,
            settle_at,
            perpetual,
            peak_cmds: peak,
            settle_signal: self.host.settle_signal(),
            notes,
        }
    }

    /// 输入结束后，`settle_signal` 首次判为"不动"的帧号。
    fn settle_point(&self) -> (Option<u32>, bool) {
        let (_, be) = self.band;
        if be >= self.frames_total {
            return (None, false);
        }
        for (i, f) in self.frames.iter().enumerate().skip(be as usize) {
            let idle = match self.host {
                Host::Hover | Host::Composite => f.frame_ms.is_none(),
                Host::Trail | Host::Click => !f.produced,
                Host::KeySound => false,
            };
            if idle {
                return (Some(i as u32), false);
            }
        }
        let perpetual = self.frames.last().is_some_and(|f| f.produced);
        (None, perpetual)
    }

    /// 判据结论 → JSON（给人和 AI 一起看）。
    pub fn to_json(&self, checks: &Checks) -> Json {
        json!({
            "host": self.host.host_global(),
            "profile": self.profile.as_str(),
            "input_band": [self.band.0, self.band.1],
            "frames_requested": self.frames_requested,
            "frames": self.frames_total,
            "verdict": if checks.passed() { "pass" } else { "fail" },
            // ★ `judged=false` = 这类脚本在本环境里**判不了**（缺位图 / 键音那种非逐帧契约）。
            //   单看 `pass` 会读成"验过了"，而它其实只是"没发现错"。
            "judged": !self.uses_image,
            "uses_image": self.uses_image,
            "checks": {
                "first_frame_draws": checks.first_draws,
                "draws_while_moving": checks.moving_draws,
                "settle_signal": checks.settle_signal,
                "settled_at_frame": checks.settle_at,
                "perpetual": checks.perpetual,
                "peak_cmds": checks.peak_cmds,
            },
            "failed": checks.failed,
            "peak_cmds": checks.peak_cmds,
            "settled_at_frame": checks.settle_at,
            "timeline": self.timeline(),
            "notes": checks.notes,
        })
    }

    /// 等距采样出一小段逐帧时间线（首帧、末帧一定在内）。
    pub fn timeline(&self) -> Vec<Json> {
        let n = self.frames.len();
        if n == 0 {
            return Vec::new();
        }
        let mut idx: Vec<usize> = Vec::new();
        let step = (n as f32 / TIMELINE_POINTS as f32).max(1.0) as usize;
        let mut i = 0;
        while i < n {
            idx.push(i);
            i += step;
        }
        if idx.last().copied() != Some(n - 1) {
            idx.push(n - 1);
        }
        idx.into_iter()
            .map(|i| {
                let f = self.frames[i];
                json!({
                    "frame": i,
                    "cmds": f.cmds,
                    "produced": f.produced,
                    "frame_request_ms": f.frame_ms,
                })
            })
            .collect()
    }
}

/// 判据结论（供调用方组装 JSON / 退出码）。
pub struct Checks {
    pub failed: Vec<&'static str>,
    pub first_draws: Option<bool>,
    pub moving_draws: Option<bool>,
    pub settle_at: Option<u32>,
    pub perpetual: bool,
    pub peak_cmds: usize,
    pub settle_signal: &'static str,
    pub notes: Vec<String>,
}

impl Checks {
    pub fn passed(&self) -> bool {
        self.failed.is_empty()
    }
}

/// 运行参数。
pub struct Opt {
    pub frames: u32,
    pub screen: (f32, f32),
    pub params: HashMap<String, Json>,
    pub host: Host,
    pub profile: Profile,
}

impl Opt {
    /// 输入区间 `[start, end)`：这段里每帧都发 `on_move`。
    pub fn band(&self) -> (u32, u32) {
        match self.host {
            // 点击效果没有"移动期"：实例由按下出生，之后只看它自己停不停。
            Host::Click => (0, 0),
            _ => match self.profile {
                Profile::Sweep => (0, sweep_frames(self.frames)),
                Profile::IdleSwing => {
                    let (idle, legs, leg) = idle_swing_band();
                    (idle, (idle + legs * leg).min(self.frames))
                }
            },
        }
    }

    /// 出图默认取哪一帧：输入区间的末帧（甩到一半 / 刚触发的那一刻，信息量最大）。
    pub fn default_shot_frame(&self) -> u32 {
        let (_, be) = self.band();
        be.max(1).min(self.frames.saturating_sub(1).max(1))
    }
}

/// `draw` 返回值 → 续帧毫秒（与 `HoverDotInstance::frame_request_ms` 同判据）。
fn frame_request_ms(v: &mlua::Value) -> Option<u32> {
    let ms = match v {
        mlua::Value::Integer(n) => *n as f64,
        mlua::Value::Number(n) => *n,
        _ => return None,
    };
    if ms.is_finite() && ms > 0.0 {
        Some(ms.clamp(1.0, 5000.0) as u32)
    } else {
        None
    }
}

/// 一个"推到第 N 帧"的驱动器：把注入环境 / 事件 / 子步 / 绘制这四步封起来，
/// `run` 与 `render_frame` 共用同一份实现（两份必然分叉）。
struct Driver {
    eng: ScriptingEngine,
    host: &'static str,
    params: HashMap<String, Json>,
    screen: (f32, f32),
    origin: (f32, f32),
    cursor: (f32, f32),
    elapsed: f32,
    click: bool,
    /// 最近一帧 `draw` 的返回值（续帧请求），由 [`Self::frame`] 填。
    last_frame_ms: Option<u32>,
}

impl Driver {
    fn new(root: &Path, id: &str, opt: &Opt) -> Result<Self, String> {
        let eng = ScriptingEngine::new(root).map_err(|e| format!("初始化 Lua 引擎失败：{e}"))?;
        eng.load_script(id).map_err(|e| format!("加载失败：{e}"))?;
        let first = plan(opt.profile, 1, opt.screen.0, opt.screen.1);
        let cursor = first.first().map(|s| (s.x, s.y)).unwrap_or((0.0, 0.0));
        let d = Self {
            eng,
            host: opt.host.host_global(),
            params: opt.params.clone(),
            screen: opt.screen,
            origin: cursor,
            cursor,
            elapsed: 0.0,
            click: opt.host == Host::Click,
            last_frame_ms: None,
        };
        if d.click {
            d.env()?;
            d.eng.set_wall_ms(0.0).map_err(|e| e.to_string())?;
            d.eng
                .call_on_button("left", d.cursor.0, d.cursor.1, 0.0, &d.params)
                .map_err(|e| format!("on_press 出错：{e}"))?;
        }
        Ok(d)
    }

    fn env(&self) -> Result<(), String> {
        self.eng
            .update_env(
                self.cursor,
                self.origin,
                self.elapsed,
                self.screen,
                self.host,
            )
            .map_err(|e| format!("注入环境失败：{e}"))?;
        self.eng
            .set_wall_ms(f64::from(self.elapsed) * 1000.0)
            .map_err(|e| e.to_string())
    }

    /// 推进一步（含可能的 `on_move`），返回本帧的 DrawList。
    fn frame(&mut self, i: u32, step: Step) -> Result<DrawList, String> {
        self.last_frame_ms = None;
        if step.moving {
            self.cursor = (step.x, step.y);
        }
        self.env().map_err(|e| format!("第 {i} 帧{e}"))?;
        if step.moving {
            self.eng
                .call_on_move(self.cursor.0, self.cursor.1, self.elapsed, &self.params)
                .map_err(|e| format!("第 {i} 帧 on_move 出错：{e}"))?;
        }
        for _ in 0..SUBSTEPS {
            self.env().map_err(|e| format!("第 {i} 帧{e}"))?;
            self.eng
                .call_step(DT, self.elapsed, &self.params)
                .map_err(|e| format!("第 {i} 帧 step 出错：{e}"))?;
            self.elapsed += DT;
        }
        self.env().map_err(|e| format!("第 {i} 帧{e}"))?;
        let mut list = DrawList::new();
        api_draw::set_draw_target(&mut list);
        let ret = self.eng.call_draw(&self.params);
        api_draw::clear_draw_target();
        let ret = ret.map_err(|e| format!("第 {i} 帧 draw 出错：{e}"))?;
        self.last_frame_ms = frame_request_ms(&ret);
        Ok(list)
    }
}

/// 跑一遍。`root` = lua 根目录，`id` = 相对根的脚本 id（不含 `.lua`）。
///
/// 返回 `Err` = 加载失败或**任一帧抛了 Lua 错误**（带帧号与原始报错，给 AI 改）。
pub fn run(root: &Path, id: &str, opt: &Opt) -> Result<Report, String> {
    if opt.host == Host::KeySound {
        return Err(
            "键音脚本的契约是 `run(ctx, params)`（按键触发一次），不是逐帧绘制，本工具驱动不了它。             要给它写自动判据，请改用 `scripts/` 里的复合脚本（它收 on_key）。"
                .into(),
        );
    }
    let src = std::fs::read_to_string(root.join(format!("{id}.lua"))).unwrap_or_default();
    let uses_image = src.contains(":image(") || src.contains(":image (");

    // 帧预算：拖尾只看给定窗口（持续性动画是合法的）；其余三类必须**给出收敛结论**，
    // 所以输入结束后额外给 SETTLE_BUDGET 帧，一旦收敛立刻停（不白跑）。
    let (_, be) = opt.band();
    let cap = match opt.host {
        Host::Trail => opt.frames.max(be),
        _ => opt.frames.max(be + SETTLE_BUDGET),
    };
    let steps = plan(opt.profile, cap, opt.screen.0, opt.screen.1);

    let mut d = Driver::new(root, id, opt)?;
    let mut frames: Vec<Frame> = Vec::with_capacity(opt.frames as usize);
    for i in 0..cap {
        let list = d.frame(i, steps[i as usize])?;
        let cmds = list.len_all();
        let frame = Frame {
            cmds,
            produced: cmds > 0,
            frame_ms: d.last_frame_ms,
        };
        frames.push(frame);
        // 输入一停，一旦本宿主判为"不动"就收工——再跑下去只是空转。
        if i >= be {
            let idle = match opt.host {
                Host::Hover | Host::Composite => frame.frame_ms.is_none(),
                Host::Trail | Host::KeySound => false,
                Host::Click => !frame.produced,
            };
            if idle && i > be {
                break;
            }
        }
    }

    Ok(Report {
        host: opt.host,
        profile: opt.profile,
        frames_total: frames.len() as u32,
        frames_requested: opt.frames,
        band: opt.band(),
        frames,
        uses_image,
        notes: Vec::new(),
    })
}

/// 跑 `at` 帧并把那一帧画成位图（预乘 RGBA，与 windui 的 `DibTarget` 同源）。
///
/// `scale` 把逻辑像素压到画布尺寸（默认 0.5 → 1920×1080 的桌面画进 960×540）。
pub fn render_frame(
    root: &Path,
    id: &str,
    opt: &Opt,
    at: u32,
    canvas: (u32, u32),
    scale: f32,
) -> Result<Vec<u8>, String> {
    use windui::render::{DibTarget, RenderTarget};
    use windui::text::DWriteEngine;

    let cap = at.saturating_add(1).max(2);
    let steps = plan(opt.profile, cap, opt.screen.0, opt.screen.1);
    let mut d = Driver::new(root, id, opt)?;
    let mut last = DrawList::new();
    for i in 0..cap {
        last = d.frame(i, steps[i as usize])?;
    }

    let (cw, ch) = canvas;
    let mut buf = backdrop(cw, ch);
    {
        // 作用域是必须的：`DibTarget` 借走了 `buf`，不先放开就没法把它移出去返回。
        let mut target = DibTarget::new(&mut buf, cw, ch);
        let mut engine = DWriteEngine::new();
        let mut cv = target.make_canvas(&mut engine, scale);
        crate::runtime::render::translate_offset(&last, &mut *cv, scale, 0.0, 0.0);
    }
    Ok(buf)
}

/// 背景：左深右浅 + 四块卡片。
///
/// 不只为了好看：亮度差是**对比度判据**的唯一来源——同一个特效在深色和浅色桌面上必须
/// 都看得见（这是"浅壁上几乎看不见"那类反馈的复现面）。
fn backdrop(w: u32, h: u32) -> Vec<u8> {
    let mut buf = vec![0_u8; (w * h * 4) as usize];
    for y in 0..h {
        let t = y as f32 / h as f32;
        for x in 0..w {
            let (r, g, b) = if x < w / 2 {
                (
                    (44.0 + 24.0 * t) as u8,
                    (54.0 + 28.0 * t) as u8,
                    (70.0 + 30.0 * t) as u8,
                )
            } else {
                (
                    (196.0 + 30.0 * t) as u8,
                    (204.0 + 26.0 * t) as u8,
                    (214.0 + 22.0 * t) as u8,
                )
            };
            let i = ((y * w + x) * 4) as usize;
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = 255;
        }
    }
    let card = |buf: &mut Vec<u8>, cx: u32, cy: u32, cw: u32, chh: u32, c: (u8, u8, u8)| {
        for y in cy..(cy + chh).min(h) {
            for x in cx..(cx + cw).min(w) {
                let i = ((y * w + x) * 4) as usize;
                buf[i] = c.0;
                buf[i + 1] = c.1;
                buf[i + 2] = c.2;
                buf[i + 3] = 255;
            }
        }
    };
    card(&mut buf, 40, 40, 260, 110, (248, 249, 252));
    card(&mut buf, 40, 170, 260, 110, (32, 36, 44));
    card(&mut buf, w / 2 + 60, 60, 200, 90, (250, 250, 250));
    card(&mut buf, w / 2 + 60, 300, 200, 90, (28, 30, 38));
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(host: Host, frames: u32) -> Opt {
        Opt {
            frames,
            screen: (1920.0, 1080.0),
            params: HashMap::new(),
            host,
            profile: Profile::Sweep,
        }
    }

    /// 两种输入轮廓的区间形状：sweep 从第 0 帧就开始动；idle-swing 必须**先静一段**再动
    /// （否则「晃动定位」永远不会被武装，工具就会把一个正确的脚本判红）。
    #[test]
    fn input_bands_match_their_profiles() {
        let sweep = opt(Host::Hover, 300);
        assert_eq!(sweep.band(), (0, 90));

        let mut swing = opt(Host::Hover, 300);
        swing.profile = Profile::IdleSwing;
        let (s, e) = swing.band();
        assert!(
            s >= 60,
            "idle-swing 前 {s} 帧不该动（要让 idle_s=1.0s 成立）"
        );
        assert!(e > s && e <= 300);

        // 点击宿主没有"移动期"：实例由按下出生。
        assert_eq!(opt(Host::Click, 300).band(), (0, 0));
    }

    /// 判据必须能红：首帧不画 + 永不停帧 → 两条都进 `failed`。
    #[test]
    fn checks_flag_first_frame_and_never_settling() {
        let frames: Vec<Frame> = (0..40)
            .map(|i| Frame {
                cmds: if i == 0 { 0 } else { 3 },
                produced: i != 0,
                frame_ms: Some(16),
            })
            .collect();
        let rep = Report {
            host: Host::Hover,
            profile: Profile::Sweep,
            frames_total: 40,
            frames_requested: 40,
            band: (0, 10),
            frames,
            uses_image: false,
            notes: Vec::new(),
        };
        let c = rep.checks();
        assert!(!c.passed());
        assert!(c.failed.contains(&"first_frame_draws"));
        assert!(c.failed.contains(&"settles"));
    }

    /// 真脚本端到端：出厂「实心圆」必须 pass（它是"静止就不动"的纯静态悬浮标）。
    #[test]
    fn shipped_static_hover_script_passes() {
        let root = crate::core::paths::lua_ext_dir();
        let rep = run(&root, "HoverDot/solid", &opt(Host::Hover, 120)).expect("跑通");
        let c = rep.checks();
        assert!(
            c.passed(),
            "实心圆应判 pass，实际失败 {:?}（{:?}）",
            c.failed,
            c.notes
        );
        assert!(c.first_draws == Some(true));
    }

    /// 反例走真链路：写一个"首帧不画"的脚本，`run` 必须把它判红而不是放行。
    #[test]
    fn bad_script_gets_rejected() {
        let dir = std::env::temp_dir().join(format!("curseen_trace_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("HoverDot")).expect("建目录");
        std::fs::write(
            dir.join("HoverDot/bad.lua"),
            concat!(
                "name = \"bad\"\nrole = \"effect\"\n",
                "local age = 0\n",
                "function step(dt) age = age + dt end\n",
                "function draw(ctx, p)\n",
                "  if age < 0.2 then return 16 end\n",
                "  ctx:circle(mouse_x, mouse_y, 8, 255, 0, 255, 255)\n",
                "  return 16\n",
                "end\n",
            ),
        )
        .expect("写脚本");
        let rep = run(&dir, "HoverDot/bad", &opt(Host::Hover, 120)).expect("跑通");
        let c = rep.checks();
        assert!(
            c.failed.contains(&"first_frame_draws"),
            "首帧不画必须判红，实际 {:?}",
            c.failed
        );
        assert!(
            c.failed.contains(&"settles"),
            "永不停帧必须判红，实际 {:?}",
            c.failed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 图片类脚本在离屏里零指令是**正确**的（没有位图），不许判 fail。
    ///
    /// 这条曾经是红的：`HoverDot/png` 是出厂脚本，工具却报 `draws_at_all` fail ——
    /// 外部 AI 会照着"修一个没坏的脚本"，把判据自己的锅算到脚本头上。
    #[test]
    fn image_script_without_bitmap_is_not_a_failure() {
        let frames: Vec<Frame> = (0..30)
            .map(|_| Frame {
                cmds: 0,
                produced: false,
                frame_ms: None,
            })
            .collect();
        let rep = Report {
            host: Host::Hover,
            profile: Profile::Sweep,
            frames_total: 30,
            frames_requested: 30,
            band: (0, 10),
            frames,
            uses_image: true,
            notes: Vec::new(),
        };
        let c = rep.checks();
        assert!(c.passed(), "没有位图不是脚本的错：{:?}", c.failed);
        assert!(
            c.notes.iter().any(|n| n.contains("ctx:image")),
            "必须说明为什么跳过，否则读者会以为「零指令也能过」是判据松了"
        );
        // 同样的账本，非图片脚本必须照样判死（防止这一改把真 bug 也放过了）。
        let mut plain = rep.clone();
        plain.uses_image = false;
        assert!(plain.checks().failed.contains(&"draws_at_all"));
        // 给 AI 的信号：pass 但 judged=false = "没发现错"，不是"验过了"。
        assert_eq!(rep.to_json(&c)["judged"], false);
    }

    /// 键音脚本不是逐帧脚本：必须明确报错，而不是跑出个"零指令"的假 fail。
    #[test]
    fn keysound_script_is_refused_with_a_reason() {
        let root = crate::core::paths::lua_ext_dir();
        let err =
            run(&root, "KeySound/unified_sound", &opt(Host::KeySound, 60)).expect_err("应明确拒绝");
        assert!(err.contains("run(ctx"), "报错要说清契约，实际：{err}");
    }
}
