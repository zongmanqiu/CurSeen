# windui Patch Notes — CurSeen (可寻) · **0.30.0**

> **本文件是 `.3rdParty/wind-ui-rust-0.30.0` 的补丁登记表。** 历史章节（0.19 / 0.20 时代）
> 原样承接 `../wind-ui-rust-0.20.0/PATCHES.md`，只增不减：新补丁续在文末，历史章节永不删节。
> 下方「迁移说明：0.20.0 → 0.30.0」是本次升级的逐项判决；历史章节正文**不改写**，
> 与现状不符处以迁移说明为准。

> **本文件于 2026-09-23 从 `main/PATCHES-CURSEEN.md` 迁到此处**（`patches/<库名>-<版本>/`
> 组织方式，见 `patches/README.md`）。内容一字未改。
>
> **历史条目的路径已过时**，读到时请按此换算：
> `tools/deploy.py` / `main/tools/deploy.py` → `build/deploy.py`（`tools/` 整个并入
> `main/build/`）；`tools/probes/*.rs` → `examples/*.rs`；
> `main/开发指引.md §N` → `main/docs/DESIGN.md §N`（设计结论）。当年与它并列的「落地记录」
> 那一半已收敛为 `main/docs/踩坑记录.md`（按主题记坑与教训，不再是逐节编年）。
>
> 早年引用的 `notes/开发记录.md`、`main/docs/archive/**`、`snap-pin-plan-20260916.md` /
> `snap-pin-implemented-20260916.md` **均不在仓库中**（照旧路径去找一定扑空）；补丁 023
> 相关的完整实测与结论改注在 `tests/probes/freeze_overlay_poc.rs` 的用例注释里。
>
> ★ **本文件里有两个 `§` 体系，别混**：小节标题写的是「补丁 NNN」（本文件自己的序号，随
> windui 升级递增）；而**正文里出现的 `§N` / `§N.M` 一律指上游 windui 文档的编号**
> （例：`§12.21` / `§11.3` / `§4.2`）。两者毫无对应关系——查上游原话要去 windui 那一份，
> 别拿本文件的章节号去比。`§4.2` 是例外：它是**本项目计划书**的编号（该文书已不在仓库，见上）。
>
> **编号继续递增，不要删节**——升级 windui 时这是唯一的重放依据。

---

## 迁移说明：0.20.0 → 0.30.0（2026-10-24）

**做法**：`.3rdParty/wind-ui-rust-0.20.0`（**补丁后**的树）为 current、**干净** 0.20 为祖先
（temp 快照）、**干净** 0.30 为 other，逐文件 `git merge-file --diff3` 三方合并，把「我们的
改动」与「上游 0.20→0.30 的改动」精确分离。踩过的坑：**current 绝不能误用成干净 0.20**——
第一次合并拿错了 base，产物是"纯 0.30"（`grep -c CurSeen` = 0 当场暴露）。合并后
`win32/mod.rs` 有 15 个人工冲突块（上游重构了建窗流程 / `frame_size_for_client` 签名），
其余文件零冲突或小改。19 个改动文件 + 新增 `src/platform/win32/layered.rs`（补丁 001 整文件）
已归档于本目录，`3_patch.bat` 单向覆盖到 `.3rdParty/`。

### 逐项判决

| 判决 | 补丁 | 处置 |
|---|---|---|
| ❌ **作废（上游收编）** | **011** | 上游原生 `claim_channel_owner`（sync.rs + app/mod.rs 首帧认领）。我们删掉重复定义；0.30 复核时连带删除了 `channel_owner_host_id()`（原本是给 010 的自报唤醒用的 accessor，随 010 一并作废，见下） |
| ❌ **作废（上游收编）** | **018 / 003** | 上游 `WindowConfig.position: Option<(i32,i32)>` 字段 + `Window::position()` / `App::position()`，语义与我们的 `pos` 相同（字段改名）。建窗坐标换算已按新字段走 |
| ❌ **作废（上游收编）** | **027** | 上游 `tray::taskbar_created_msg() -> u32`（缓存）+ `tray::is_taskbar_created(msg, registered)`，explorer 重启重建托盘图标的机制原样存在 |
| ❌ **作废（上游收编）** | **028–030** | 上游 `ui/color_picker.rs` 整体重写（2012 行），HEX 全选 / 手动输入 / 拖动实时三条语义都在（030(a) 半截输入门、030(b) Move-dirty 测试逐条对过） |
| ❌ **作废（上游收编）** | **088** | 上游 `TrayOp::SetIcon` 收 `Vec<u8>`，`TrayHandle::swap_icon(hicon)` 单参；运行期换图标能力原样存在（**app 侧调用签名见迁移说明尾部**） |
| ❌ **作废（上游收编）** | **089** | 上游收编且更完整：`scroll_edge_inset` 横竖两轴，按 `frameless && resizable` 归零（我们旧版只判 `resizable`，带框窗口会被误内缩 8px——重放时曾因此测试红，已删旧块留上游语义） |
| ❌ **作废（上游收编）** | **091** | 上游托盘菜单自带 `TPM_BOTTOMALIGN \| TPM_RIGHTALIGN` |
| ❌ **作废（上游收编）** | **094** | 上游 `on_blur` 原生提供（回调签名 `Fn(&mut EventCtx)`，与 0.20 的 `BlurFn` 不同） |
| ❌ **作废（本轮清理）** | **010** | 唤醒不再强推按帧驱动的窗口——语义已由**上游 0.30 的 `WM_APP_WAKE` 过滤**恢复（`wants_animation() && next_frame_delay_ms()==0` 的窗口跳过 `InvalidateRect`，与 010 同义）。复核时发现我们的 `wants_wake_repaint` trait 方法**自 0.20 起就没有调用点**（0.19→0.20 升级重写唤醒路径时调用点丢了，一直死代码），连同 `sync::channel_owner_host_id()`（011 的 accessor，唯一消费者就是它）一并删除；019 登记表原文保留作史 |
| 🔄 **轻改重放** | **002 / 009 / 023 / 090 / 031 / 032** | 渲染快路族原样重放；002 的 `blend` 闸仍在（上游 `fast_fill_rect` 依旧不看 blend，陷阱照旧）；023 的头截断改用上游 `text_fit::truncate_to_width(.., Truncate::Start)`，不再自带 helper |
| 🔄 **轻改重放** | **014** | 滑杆值标签宽度映射按 0.30 的 `Slider` 结构重放（`value_range: Option<(f32,f32,u8)>` + `slider_value_range()`） |
| 🔄 **轻改重放** | **015** | 上游构造器改为 `SkiaCanvas::with_text(impl IntoPixmapMut, ..)`，我们的 `PixmapMut` 契约顺势成为上游接口——只需保证 `DibTarget` 实现 `IntoPixmapMut`，改动比 0.20 版更小 |
| 🔄 **轻改重放** | **024** | `DispatchResult` 按上游新结构补 `capture: Option<Option<NodeId>>` 字段（合并了上游的字段增删） |
| ✅ **原样重放** | 001 / 004 / 006 / 007 / 008 / 012 / 013 / 016 / 017 / 022-a / 083 / 083-a / 083-b / 092 / 093 / 095 | 上游未触及或改动无关，原样承接（含 M1 `layered.rs` 整文件、`layered` feature、`Win32_UI_Input_Pointer` 特性依赖） |

### 验证状态（m11 时点）

- `cargo check`（默认 + `--features layered`）零 error 零 warning；
- `cargo test --features layered`：**1359 + 113 全过**（唯一红过的
  `scrollbar_edge_inset_follows_window_capabilities` 即上文 089 误放行旧块，删旧块后转绿）；
- **m11 语义复核（代码级，逐条对照 019/020 登记表的机制描述）**：
  渲染快路 002 闸 / 009 清屏 / 090 透明直写 / 023 免 mask 落位且各有单测；
  016（prev_fg 记录归还）/ 017（菜单线程 + `WM_TRAY_RESULT` 回投）/ 091（对齐标志）落位；
  010 结论见上表（死代码删除，语义由上游 `WM_APP_WAKE` 过滤恢复）。
  **真机探针级验证在 m13 收口**——本表"落位"是代码 + 测试级证据，不是真机测量结论。

### app 侧连带改动（m12 清单，暂记）

`WindowConfig.pos/position` 改名（`.pos(...)` → `.position(...)`）、`on_blur` 回调签名
（`BlurFn` → `Fn(&mut EventCtx)`，`take_blur` → `take_click` 一族）、`set_icon_rgba`
的 `Arc<[u8]>` → `Vec<u8>`、`scroll_window_edge_inset` → `scroll_edge_inset`。

---

## 迁移说明：0.19.0-fork → 0.20.0（2026-09-27）

> **本文件是 `.3rdParty/wind-ui-rust-0.20.0` 的补丁登记表。** 下方编号章节内容原样承接
> `../wind-ui-rust-0.19.0/PATCHES.md`（升级依据与本表一致，追溯背景两份通用）。
> 只增不减的规矩不变：新补丁续在文末，历史章节永不删节。

**做法**：从 codeload 下载**干净**的 0.19.0 / 0.20.0 原始包 → 三方合并
（`git merge-file --diff3`，以 0.19-fork 为 current、干净 0.19 为祖先、干净 0.20 为 other），
把「我们的改动」与「上游改动」精确分离。结果：22 个我们改过的文件里 **19 个零冲突自动合并**、
**3 个需人工（共 7 个冲突块）**。上游 0.20.0 的改动绝大多数是**纯新增**
（`hide_on_minimize` / `TrayAction::Toggle` / `MenuItem::checkable` / `pending_windows`），
不是重构已有签名 —— 所以冲突面天然小。

### 逐项判决

| 判决 | 补丁 | 处置 |
|---|---|---|
| ❌ **作废** | **007** | 上游把宿主后备缓冲**整块删了**（改 `last_size`，pixmap 跨帧持久），补丁目标（省整屏内存）以更彻底方式达成。`back_enabled` / `apply_partial_back` / `set_partial_back`（trait + impl + 平台调用）三处连带删除 |
| ❌ **作废** | **023** | 图像 blit 不构造整屏 mask —— 上游自做同类优化（CHANGELOG：图片页 6.4×） |
| ❌ **作废** | **031** | 实色填充快路 —— 上游 `fast_fill_rect` 覆盖且更强（含圆角 + 4×4 超采样）；其「目标全透明直写」优化以**补丁 090** 的形式补回（见文末） |
| 🔄 **轻改** | **002 / 009** | 上游快路**不看 `blend`**（签名里根本没这个参数）。`BlendMode::Clear` 若不过闸会掉进它、被当 SourceOver 混色 —— **编译通过、不报错，但画面语义直接错**。已在 `SkiaCanvas::fill_round_rect` 的上游快路口加 `paint.blend == BlendMode::SourceOver` 闸；补丁 009 的 Clear 清屏快路保留在它旁边 |
| 🔄 **轻改** | **011** | 与上游 `hide_on_minimize` 落在同一位置，**并集**（4 行拼接），补丁完全必要 |
| ✅ **保留** | 001 / 003 / 004 / 006 / 008 / 010 / 012 / 014 / 015 / 016 / 017 / 018 / 019–022 / 024 / 025–030 / 032 / 083–089 | 上游未触及或改动无关，原样承接 |
| ➕ **新增** | **090** | 见文末 |

### 三方合并需人工的 7 个冲突块

| 文件 | 冲突块 | 处置 |
|---|---|---|
| `src/app/mod.rs` | 2 | **并集**：补丁 011（建主窗即认领通道消费权）+ 上游 `hide_on_minimize` |
| `src/app/damage.rs` | 3 | **接受上游**：删后备缓冲，改 `last_size` |
| `src/render/skia.rs` | 2 | 保留补丁 009 的 Clear 快路 + 上游 `fast_fill_rect`（**加 blend 闸**）；`to_sk_blend` 与上游 `mul255` 互不相干 → 并集 |

### ★★★ 必须记住的陷阱

上游 `fast_fill_rect(x, y, w, h, radius, anti_alias, color)` **签名里没有 `blend`**，
只按 `gradient` + 几何判断（实测 0.20.0 的 `Paint` 无 `blend` 字段、无 `BlendMode` 枚举）。
`BlendMode::Clear`（清屏）若落进它，会被当 SourceOver 混色 —— **不报错、编译通过、
画面语义错**。闸口在 `SkiaCanvas::fill_round_rect`，改动时务必带着它。

### 另一处适配（非补丁）

本 fork 的 `SkiaCanvas::target_pixmap()` 返回 `Option<PixmapMut>`（补丁 015 让主缓冲可能是
外部内存的 DIB），而上游新增的 `fast_fill_rect` 假定它返回 `&mut Pixmap`。已在
`fast_fill_rect` 里改成 `let Some(mut target) = ... else { return false }`（拿不到就落回通用路径）。

---

All patches are additive and guarded by feature flags. No existing API is changed.

---

## M1: Layered Overlay Window

### Objective
Add a `LayeredBackend` that renders Skia → DIB → `UpdateLayeredWindow`, giving windui
transparent, always-on-top, click-through-capable windows without a visible frame.

### New Files

#### `src/platform/win32/layered.rs`

```
pub(crate) struct LayeredBackend {
    pixmap: Option<Pixmap>,
    buf_w: i32,
    buf_h: i32,
    fresh: bool,
    dib_section: HBITMAP,
    dib_bits: *mut c_void,
    dib_w: i32,
    dib_h: i32,
    click_through: bool,
}
```

**Core logic:**
- `LayeredBackend::new()` → HBITMAP via `CreateDIBSection` (BI_RGB, 32bpp, top-down).
- `ensure_size(w, h)` → `DeleteObject` old HBITMAP + `CreateDIBSection` new; set `fresh = true`.
- `paint(hwnd, bg, handler)` → Skia render to `Pixmap`, swap RB, `UpdateLayeredWindow`.
- `set_click_through(hwnd, enabled)` → `SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ... | WS_EX_TRANSPARENT | WS_EX_LAYERED)` or remove `WS_EX_TRANSPARENT`.

**Key invariant:** `BeginPaint`/`EndPaint` are NOT called — `UpdateLayeredWindow` is self-contained.
The window must have `WS_EX_LAYERED` at creation time.

> ⚠️ **修正（补丁 003）**：正因为不走 `BeginPaint`/`EndPaint`，分层后端 **必须** 在
> `UpdateLayeredWindow` 成功后自行调用 `ValidateRect(hwnd, None)` 清空更新区。
> 否则窗口永远处于 invalid 状态，`PeekMessageW` 每轮都合成一条 `WM_PAINT`，
> 消息泵的非阻塞排空循环会死转（实测 ≈ 8.8 万次/秒），导致空闲时单核 ~82% CPU。
> 见文件末尾「补丁 003」。

---

### Modified Files

#### `src/platform/win32/mod.rs`

**Change 1: Backend selection** (line ~760 in `WindowState::new`)
```rust
// BEFORE
backend: Box::new(SkiaBackend::new()),
// AFTER
backend: Box::new(layered::LayeredBackend::new()),
```
Guarded by `#[cfg(feature = "layered")]`. Non-layered builds unchanged.

**Change 2: Window style** (line ~920 in `create_window`)
```rust
// BEFORE
WINDOW_EX_STYLE::default(),
// AFTER
{
    #[cfg(feature = "layered")]
    {
        if cfg.layered {
            WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOPMOST.0)
        } else {
            WINDOW_EX_STYLE::default()
        }
    }
    #[cfg(not(feature = "layered"))]
    WINDOW_EX_STYLE::default()
}
```

**Change 3: WM_PAINT guard** (line ~1431)
When layered, skip `BeginPaint`/`EndPaint` — `LayeredBackend::paint` handles its own presentation.
```rust
WM_PAINT => {
    if let Some(state) = state_from(hwnd) {
        #[cfg(feature = "layered")]
        if state.handler.layered() {
            state.paint(hwnd);
            return LRESULT(0);
        }
        state.paint(hwnd);
    }
    // ... existing apply_window_op etc.
}
```

**Change 4: WM_ERASEBKGND** (already returns 1 — no change needed for layered).

---

#### `src/app/mod.rs`

**Add fields to `WindowConfig`:**
```rust
pub layered: bool,
pub click_through: bool,
```
Default both to `false`.

**Add builder methods to `App`:**
```rust
pub fn layered(mut self) -> Self { self.cfg.layered = true; self }
pub fn click_through(mut self) -> Self { self.cfg.click_through = true; self }
```

---

#### `src/core.rs` / `src/event.rs`

**Add `WindowOp` variant:**
```rust
SetClickThrough(bool),
```

**Add `EventCtx` method:**
```rust
pub fn set_click_through(&mut self, enabled: bool) {
    self.out.window_op = Some(WindowOp::SetClickThrough(enabled));
}
```

**Handle in `run_window_op`:**
```rust
Some(WindowOp::SetClickThrough(enabled)) => {
    layered::toggle_click_through(hwnd, enabled);
}
```

---

### Spike Example

#### `examples/curseen_overlay_spike.rs`

Minimal proof-of-concept: transparent always-on-top window with click-through toggle via `F12`.

```rust
use windui::prelude::*;

fn main() {
    App::new("CurSeen Overlay Spike", 400, 300)
        .layered()
        .click_through()
        .frameless()
        .content(
            Element::col().fill().child(
                Element::label("透明覆盖层 (F12 切换穿透)").font_size(14.0)
            )
        )
        .on_key_down(|ctx, key| {
            if key.code == KeyCode::F12 {
                let click = ctx.window_state().click_through;
                ctx.set_click_through(!click);
            }
        })
        .run();
}
```

---

### Performance (M1-8): Partial Redraw

`LayeredBackend` uses a persistent DIB section. `UpdateLayeredWindow` accepts a
bounding rectangle (`pptFrom`), so only the dirty region is recomposited. The DIB
content persists between frames — Skia renders to Pixmap, RB-swap only the dirty rect
into the DIB, then `UpdateLayeredWindow` with `prcDirty = Some(dirty_rect)`.

**GDI resource limits:** One DIB section per window. `DeleteObject` on resize. No
`CreateCompatibleDC` per frame — cache the memory DC.

---

### Fallback Strategy

If M1 spikes fail (WS_EX_LAYERED + Skia incompatible, GDI clipping issues, etc.):
- **Plan C:** Build the overlay from scratch using raw Win32 API + skia-safe directly,
  bypassing windui's windowing layer entirely. The windui patch is abandoned but the
  `App` builder API stays identical for user-facing code.

---

## 补丁 002: 给 Canvas 补混合模式（`BlendMode`：Plus 加色叠加 / Clear 抹脏区）

> **补登说明（2026-09-17）**：本补丁当初在源码里留了 `**CurSeen 补丁 002**` 标记，却没有在
> 本文档登记章节——登记表从 003 起才开始。遗漏是由新增的文档门禁
> `main/tests/docs_consistency.rs::test_patch_record_has_a_section_for_every_windui_patch`
> 扫出来的（正是 Pecia 的 `test_docs` 想防的那一类"改了第三方却没归档"）。此处据现存源码补记。

### 落点
`.3rdParty/wind-ui-rust-0.16.1/src/render/mod.rs`

- `pub enum BlendMode { SourceOver, Plus, Clear }`（默认 `SourceOver`，与补齐前行为一致）
- `Paint` 新增 `pub blend: BlendMode` 字段，并提供 `Paint::fill_blend(color, blend)` 构造入口

### 为什么必须补这两个模式

上游 windui 的 `Canvas` 只支持 `SourceOver`，而 CurSeen 有两类操作它根本表达不了：

| 需求 | 缺什么 | 后果 |
|---|---|---|
| 粒子 / 辉光高光 | `Plus`（`dst = min(dst + src, 1)`） | SourceOver 叠多少层都只会**趋近于画笔色**，叠不出"越亮"的高光 |
| 分层透明窗每帧抹脏区 | `Clear`（把目标像素**含 alpha** 抹成 0） | 后端帧间不清缓冲，而 SourceOver + 透明色按 alpha 合成**擦不掉任何东西**（结果等于原样保留），于是拖尾残留永不消失 |

映射到 tiny-skia 的 `BlendMode`（0.12 三者均支持），框架侧不需要自己写像素循环。

### 验收（可复现）

| 判据 | 手段 | 结果 |
|---|---|---|
| 归档完整性 | `tests/docs_consistency.rs::test_patch_record_has_a_section_for_every_windui_patch` | ✓ 由红转绿 |
| 编译与既有测试 | `cargo test --release` | ✓ 447 passed |

---

## 补丁 003: 分层后端空闲 CPU 归零（ValidateRect）

### 问题（P7 验收指标之一：静止 0% CPU 未达成）
实测空闲（无任何鼠标/按键活动）时 `curseen.exe` 单核占用 **~82%**，但 `tools/thread_cpu.ps1`
定位到 UI 主线程（持有 `WindUiWindowClass` 顶层窗口）100% Running，且 `idle_probe.py` 检测到该
窗口 `Invalid=True`（更新区永不被校验）。进一步在 vendored windui 消息泵加诊断，确认
`LayeredBackend::paint` 在 10 秒内被调用 **~878,000 次**（≈ 8.8 万次/秒）。

### 根因
分层后端走 `UpdateLayeredWindow` 自行呈现，**不调用 `BeginPaint`/`EndPaint`**（见 M1 不变式）。
但 windui 的消息泵用 `PeekMessageW(..., PM_REMOVE)` 非阻塞排空循环：只要窗口仍 invalid，
`PeekMessageW` 就会合成一条 `WM_PAINT` 并再次进入 `paint` → `UpdateLayeredWindow` → 又 invalid
→ 无限循环。普通 Skia 后端靠 `BeginPaint` 隐式 `ValidateRect` 打破这个环；分层后端缺了这一步。

### 修复
在 `LayeredBackend::paint` 的 `UpdateLayeredWindow` 调用 **之后** 立即 `ValidateRect(hwnd, None)`
清空整窗更新区；同时恢复 `WS_EX_TRANSPARENT` 之前先 `ValidateRect` 避免残留脏区。
（`win32/layered.rs`：`use windows::Win32::UI::WindowsAndMessaging::ValidateRect;`）

> 注意：**不是** `BeginPaint`/`EndPaint`，是 `ValidateRect`。后者不进入 PAINTSTRUCT、不取 HDC，
> 与「不走 Begin/End」的不变式不冲突。

### 验收
- `tools/idle_probe.py` 10 秒采样：全线程 `State=Wait`，`IdleCpuPercent = 0.00`。
- `tools/measure.ps1`：`IdleCpuPercent` 0.00%（12 核环境），窗口 `Invalid=False`。
- 绘制逻辑未变：有内容时仍按帧续帧（`request_repaint_in`），空内容时不续帧 → 真·零 CPU 空闲。

### 负面影响
无功能性回归。仅修正了"更新区永不清空"的副作用，帧语义不变。

---

## 补丁 004: 分层覆盖层必须无边框（WS_POPUP）

### 问题（用户实测：双击 exe 后"空窗口 + 标题栏 + 关不掉 + 挡任务栏"）
启动 `curseen.exe` 后看到一个大得盖住任务栏的空窗口，带着标题栏，点右上角 × 关不掉。
这正是覆盖层主窗——它本应是**看不见的透明、鼠标穿透、全屏**浮层，却以带标题栏的普通
窗口形态出现。

### 根因
`create_window`（`src/platform/win32/mod.rs`）里窗口样式只按 `cfg.resizable` 算：
```rust
let mut win_style = if cfg.resizable {
    WS_OVERLAPPEDWINDOW
} else {
    WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 & !(WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0))
};
```
**无论是否分层，都保留了 `WS_CAPTION`（标题栏）/ `WS_SYSMENU` / `WS_MINIMIZEBOX`。**
分层只改了 `ex_style`（加上 `WS_EX_LAYERED | WS_EX_TOPMOST`）。于是全屏覆盖层带着标题栏
出现：视觉上"空"（透明但框还在）、占满屏挡住任务栏、点 × 无 `on_close_request` 故关不掉。

附带：`frame_size_for_client` 内部写死 `WS_OVERLAPPEDWINDOW` 反算外框，把标题栏/边框厚度
也算进覆盖层尺寸，使覆盖层比虚拟屏大出一圈（透明故无害，但不精确）。

### 修复
1. `create_window`：分层窗口强制无边框弹出式。
   ```rust
   #[cfg(feature = "layered")]
   if cfg.layered {
       win_style = WS_POPUP;   // 去掉标题栏/系统菜单/最小化按钮
   }
   ```
2. `frame_size_for_client` 增加 `style: WINDOW_STYLE` 形参，两处调用（建窗反算 + DPI 校正）
   都传真实 `win_style`；`WS_POPUP` 经 `AdjustWindowRectExForDpi` 不增边框 → 覆盖层精确铺满虚拟屏。
3. `WM_GETMINMAXINFO`：分层窗口走与 `frameless` 同款的"客户区即外框"分支，最小尺寸精确。

> 语义：分层窗口 = 透明绘制面，本就不该有 chrome（标题栏）。所有分层窗（覆盖层、toast、
> 浮层）统一无边框，符合 `WS_EX_LAYERED` 的标准用法。

### 配套（CurSeen 侧）
覆盖层变无边框后肉眼不可见，用户双击 exe 看不到任何窗口会困惑。故 `ui::overlay_window::run_overlay`
在 `app.run()` 前经 UI 通道发 `UiMsg::OpenPanel`，**启动即打开控制面板**（居中、800×600、
`single("panel")` 幂等）。覆盖层继续在后台透明运行（鼠标特效等照常），面板可关、可经托盘重开，
退出走托盘"退出"。

### 验收
- 启动后：控制面板以 800×600 居中出现；屏幕上无"大空窗/标题栏"；任务栏可见可点（覆盖层
  透明 + 鼠标穿透）。
- 面板标题栏 × 可正常关闭（落到 `on_close_request → Runtime::close_panel`，返回 `true`）；
  托盘"退出"经 `ctx.quit()` 结束进程。
- 覆盖层仍 `WS_EX_LAYERED | WS_EX_TOPMOST`，鼠标特效正常绘制于其上。

---

## 补丁 006: `tool_window` 选项 — 消除任务栏双图标

### 问题

启动 CurSeen 后任务栏出现 **2 个图标**：

1. 覆盖层主窗：`App::new(...).layered()` → `WS_POPUP` + `WS_EX_LAYERED|WS_EX_TOPMOST`。
   它没有 owner，也不带 `WS_EX_TOOLWINDOW`，Windows 仍会给它一个任务栏按钮——
   即使窗口透明不可见。
2. 控制面板：`open_window` 打开的普通 `WS_OVERLAPPEDWINDOW`，正常占据第二个按钮。

### 修复（windui 侧，向后兼容的新增选项）

1. `WindowConfig` 新增 `pub tool_window: bool`（默认 `false`）。
2. `create_window` 组装 `ex_style` 时：`if cfg.tool_window { bits |= WS_EX_TOOLWINDOW.0 }`。
3. `App` builder 新增 `pub fn tool_window(mut self) -> Self`。

`WS_EX_TOOLWINDOW` 语义：不进任务栏、不进 Alt-Tab——正是托盘常驻类应用
（覆盖层 + 面板）所需的窗口属性，且不影响窗口的显示、层级与输入行为。

### 配套（CurSeen 侧）

`ui::overlay_window::run_overlay` 的覆盖层构造追加 `.tool_window()`。
结果：任务栏只剩控制面板 1 个图标；覆盖层退居幕后，由托盘图标代表进程常驻。

### 验收

- 启动后任务栏仅 1 个图标（控制面板）；Alt-Tab 不出现覆盖层。
- 关闭面板后任务栏 0 图标，托盘图标仍在，可经托盘重开面板、退出进程。
- 覆盖层特效、鼠标穿透行为不变。
- 门禁：`cargo fmt` ✅ / `cargo test --lib` **240 passed, 0 failed** ✅ /
  `cargo clippy --all-targets -- -D warnings` ✅ exit 0 /
  `cargo build --release` ✅ exe **2,663,424 B (2.60 MB)**。

---

## 补丁 006 附带修复: 默认功能全部关闭 + 空闲 CPU

### 问题

`Runtime::new` 曾强制 `config.trail.trail_enabled = true; config.global.enabled_trail = true`
（"一启动就有可见特效"），后果有二：

1. **违背"所有功能默认关闭"**：配置默认（`config.rs` 三开关全 false）被启动代码覆盖，
   且被持久化进 `%APPDATA%\CurSeen\config.json`，重启后仍开着。
2. **空闲 CPU 5%**：trail 开着时，鼠标移动即产生活动特效 → `frame_demand()` 返回
   `FrameDemand::Now` → 共享消息循环以 ~60fps 空转（覆盖层 + 面板一起）。静止时虽 Idle，
   但日常使用中鼠标一动就进入动画态，任务管理器表现为持续 ~5%。

### 修复

1. 删除 `Runtime::new` 中的两行强制赋值——让配置默认（全 false）生效。
2. 重置本机 `%APPDATA%\CurSeen\config.json` 中的 `trail_enabled` / `enabled_trail` 为
   `false`（旧 bug 已把 true 持久化，仅改代码对老配置无效）。

trail 关闭时 `sync_settings` 会置 `effect = None`（controller.rs:106-108），鼠标移动
不再产生活动特效 → 覆盖层全程 `FrameDemand::Idle` → 消息循环走 `GetMessageW` 阻塞
分支 → **空闲 CPU = 0%**（含鼠标移动期间）。

### 验收

- 首启（或重置配置后）所有功能关闭：无拖尾、无按键显示、无屏幕书写。
- 鼠标任意移动，CPU 占用 ≈ 0%（此前为 ~5%）。
- 面板开启任一功能后行为照旧（`config.json` 同步持久化开启状态）。

---

## 补丁 006 附带修复: 输入门 — 鼠标事件不再唤醒空帧

### 问题（实测驱动定位）

删除强制开启 + 重置配置后，静止 CPU = 0%，但**实测发现鼠标移动期间仍会逼出空帧**：
windui 的 `Sender::send` 每条消息都会唤醒 UI 出一帧——桥接线程把**每个**鼠标事件转投
UI 通道，功能全关时这些帧是纯空帧（覆盖层整窗 clear + 全屏 DIB 上传 ~5ms/帧）。鼠标
60–120 事件/秒 → 持续 30–60 帧/秒的全屏空帧 → 任务管理器平均后正是用户看到的 ~5%。

### 修复

1. `Runtime::input_needs_frame()`：判据读 `config.global.{enabled_trail,enabled_key,
   enabled_draw}`（**不读** `state.modules.*`——`sync_panel` 只 diff 回 Config 不同步
   modules，modules 会滞后，会把面板刚打开的功能错关在门外）。
2. `app::runtime::INPUT_NEEDS_FRAME: AtomicBool`（跨线程原子门）：`Runtime::new` 与
   每帧收尾（`frame()` 尾）同步。
3. `spawn_input_bridge`：鼠标事件（Move/Down/Up/Wheel）在门关时**直接丢弃**，不入队
   不唤醒；键盘事件不设门（低频，且面板热键/按键显示依赖键盘通路）。

### 验证（tools/ 下诊断脚本）

- `moving_cpu.py`：角落移动 **0.00%**、面板上移动 **0.00%**、静止 **0.78%**（修复前
  合成 33Hz 移动 = 72–89% 单核）。
- `thread_cpu.py`（NtQuerySystemInformation 线程级采样）：干净空闲进程总 CPU 0.00%。
- `trace_loop.py`（消息循环计数）：全程 `animating=false`、0 帧——消息泵空闲阻塞。
- 门禁：fmt ✅ / 240 tests ✅ / clippy -D warnings exit 0 ✅ / build exe 2,664,448 B。



---

## 补丁 007: 覆盖层后备缓冲按需分配（空载内存 39.5 MB → ~16 MB）

### 问题（实测分解）

用户反馈：默认所有功能全关，空载私有内存仍有 ~40 MB；同框架的普通小程序只有 5 MB。

用「改窗口尺寸」做对照实验，把 1920×1080 的开销拆开（私有内存 / 工作集）：

| 组合 | Private | WS |
|---|---|---|
| 全屏覆盖层 + 开面板 | 39.50 MB | 56.94 MB |
| 8×8 覆盖层 + 开面板 | 15.61 MB | 33.10 MB |
| 全屏覆盖层 + 不开面板 | 31.06 MB | 47.04 MB |

由此解出三块：**引擎地板 ≈ 7.2 MB**、**控制面板（1200×720）≈ 8.4 MB**、
**全屏覆盖层 ≈ 23.9 MB（占 60%）**。

而 1920×1080 单层缓冲只有 7.91 MB——说明覆盖层拿了 **3 份**整屏缓冲：

1. `LayeredBackend.pixmap`（tiny-skia）；
2. `LayeredBackend` 的 DIB section（GDI）；
3. **白白多出的两份**：
   - 建窗时 `CreateWindowExW` 同步派发 WM_SIZE → `backend.resize()` → 那时后端还是
     `SkiaBackend`，按整屏尺寸先分配一份 Pixmap，下一行才被 LayeredBackend 顶掉。
     那份虽然立即 drop，但堆管理器多半只是留在进程里待复用，私有内存上看不出释放。
   - 宿主的 `damage.back`（局部重绘用的「上一帧」整屏副本）——分层后端**永远不会**
     走局部重绘（整窗 `UpdateLayeredWindow` 呈现），这份副本写了也没人读。

### 修复

1. **`layered.rs`：新增休眠（park）机制。** `PARKED: AtomicBool` + `set_parked/is_parked`
   （经 `platform::{set_overlay_parked, overlay_is_parked}` 导出）。休眠时 `ensure()`
   不分配；`paint()` 先整窗刷透明再 `release()` 掉 Pixmap + DIB，不再调用
   `UpdateLayeredWindow`。
2. **休眠也必须跑 `handler.render`。** 一开始的写法在休眠分支直接 return，结果
   `App::channel` 的意图**永远排不空**——windui 在 `UiHost::render` 里 drain pump，
   没有帧就没有 drain，功能全关时托盘点「打开控制面板」会永久卡死（实测：面板再也
   打不开）。改为渲染进一块固定 8×8 的草稿缓冲：**布局仍按真实窗口尺寸走**，绘制
   指令被裁掉，宿主逻辑完整、代价几百字节。
3. **`create_window`：分层窗在建窗**之前**就换成 `LayeredBackend`**，顺带
   `set_partial_back(false)`。前者让 SkiaBackend 那份整屏 Pixmap 根本不产生；后者
   关掉宿主的 `damage.back`（新增 `AppHandler::set_partial_back`，默认 true 保持既有
   行为）。`decide_repaint` 见 `back` 缺失会自动恒整窗——正是分层后端要的。
4. **`seed_back` 加尺寸校验**：源 pixmap 与窗口尺寸不符时不种入，避免草稿缓冲触发
   `copy_from_slice` 长度 panic。
5. **CurSeen 侧**：`App::on_interval(250ms)` 对账 `!input_needs_frame()`，状态翻转时
   `set_overlay_parked` + `request_repaint()` 走一次过渡帧；启动前先定初值。

### 验收（1920×1080，功能全关）

- 控制面板正常弹出（此前休眠分支的回归已修复）。
- Private **39.50 → 10.8~16.5 MB**（视面板后备缓冲是否已种入）；关掉面板 ≈ 8.4 MB。
- 开启拖尾后自动唤醒：Private 回到 ~26.6 MB，日志确认「初始状态：活跃」。
- 稳态 CPU ≈ 0.7%（与补丁前 0.2–1% 同级）。
- 门禁：fmt ✅ / 181 tests ✅ / clippy exit 0（11 条既有 warning，非本次引入）。

---

## 补丁 008: 穿透开关把分层窗模式换掉了 → 覆盖层定格在首帧（**所有鼠标特效不可见**）

### 问题（用户实测）

「当前的所有鼠标特效都无法显示」——5 个内置特效（像素尘埃 / 粒子光环 / 蛛丝缠网 /
辉光圆影 / 飘带）全部不见；而控制面板、托盘、按键显示一切正常。

### 定位（逐段二分渲染管线，每步都留可观测证据）

| 环节 | 观测 | 结论 |
|---|---|---|
| 底层鼠标钩子 | 注入 `SendInput` 后计数增长 | 事件进来了 |
| Runtime / 拖尾 | 一帧产出 400+ 条绘制指令，`anim=true` | 算法层在跑 |
| `OverlayCanvas::paint` | 被调用，bounds = 整屏 | 渲染循环在转 |
| 后端 pixmap | `pix(120,120) = [255,0,0,255]` | **像素确实写进去了** |
| `UpdateLayeredWindow` | 第 2 帧起恒返回 `E_INVALIDARG (0x80070057)` | ← 故障在这里 |

即 CurSeen 侧画得完全正确，是**呈现到屏幕**这一步失败，覆盖层停在第一帧（全透明）上。
中途一次误判值得记下：先把 `hdcDst` 从 `GetDC(hwnd)` 改成 `GetDC(None)` 后故障依旧，
说明那不是主因——真正的主因（下面的 `SetLayeredWindowAttributes`）与它同帧发生，
只有把两者分开验证才能定位。

### 根因

`SetLayeredWindowAttributes` 与 `UpdateLayeredWindow` 是**互斥的两种分层窗口模式**。
`toggle_click_through` 为了实现"鼠标穿透"调了
`SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)`，Windows 据此把窗口切到
"SetLayeredWindowAttributes 模式"，此后**每一次** `UpdateLayeredWindow` 都返回
`E_INVALIDARG`。

时序正好对上"首帧正常、之后全废"：穿透开关在启动后、第一帧 `present()` 之后设一次，
于是第一帧（全透明）正常上屏，之后永远画不上东西。

### 修复（windui 侧）

1. **`toggle_click_through` 改走扩展样式**：`SetWindowLongPtrW(GWL_EXSTYLE)` 增删
   `WS_EX_TRANSPARENT`，再 `SetWindowPos(.. SWP_FRAMECHANGED)` 让样式立即生效。
   全程不碰分层属性，窗口保持 `UpdateLayeredWindow` 模式。
2. **`present()` 的 `hdcDst` 改用屏幕 DC**（`GetDC(None)`，配 `ReleaseDC(None, ..)`）。
   MSDN 要求 `UpdateLayeredWindow` 的目标 DC 是屏幕 DC；传窗口 DC 属于误用（改动本身
   不是本 bug 的充分原因，但方向正确，一并修）。

### 附带修复（CurSeen 侧，同一次排查牵出的三处独立缺陷）

补丁只修好了"能画上屏"。剩下三处此前被它掩盖着——修完之后特效依然不对：

1. **`Clear` 被当成"脏区标记"用。** `DrawCmd::Clear` 在渲染层的语义是**把该矩形抹成
   透明**（`BlendMode::Clear`），不是"标记脏区"。
   - `pixie_dust` 每个粒子画完推一条自己的外接正方形 `Clear` → 粒子彼此密集重叠、
     后一个擦前一个，屏上一个粒子都不剩；
   - `key_display::render_key_popups` 在逐行绘制后追加整盒 `Clear` → 弹窗刚画完就被擦掉。

   两处都已删除，并各加一条回归锁（`draw_has_no_clear_command` /
   `key_popup_draw_has_no_clear`）。脏区（`core/damage.rs`）尚未接线；将来接也不该复用
   渲染指令 `Clear`，要走独立通道。
2. **翻译层把非 `Solid` 画笔一律回退成不透明白色。** `glow.rs` 用 `Paint::Radial` 做
   "中心亮、边缘透明"的辉光，翻译层不认渐变、回退成白圆 → 白圆落在白底/浅色底上
   等于不可见。`translate_offset` 现在把 `Paint::Linear/Radial` 映射到 windui 的
   `Gradient`，并按**图元自己的**包围盒归一化。
   ⚠️ 归一化半径要除**短边全长**（对齐 `sk_shader` 的 `radius * w.min(h)`）。windui
   `Gradient::radial` 的文档括注写"1.0≈半个短边"，与它自己的实现差一倍；照那句写会让
   渐变只铺到图元半径的一半，辉光边缘停在半程色上、露出一圈硬边。
3. **`DrawCmd` 的 `alpha` 与 `Blend` 被静默忽略。** 约定其实在代码里已经是一致的：
   便捷构造器（`fill_circle` / `draw_line` …）与 Lua API 一律传 `alpha: 1.0`、把透明度
   烘进颜色；只有手工拼指令的特效才用 `alpha` 表达"这一层是核心的百分之几"。
   渲染层此前两者都不读，于是
   - `pixie_dust` 的三层辉光全按 100% 不透明度平铺（该是 12% / 30%），
   - `Blend::Plus` 的加色高光退化成 `SourceOver`，

   粒子从"闪光"变成"实心色块"。现在 `alpha` 作为**乘子**乘在颜色自带 alpha 上、
   `Blend::Plus` 映射到 windui 的加色模式；同时把 `pixie_dust` 里 `alpha_f * 0.12`
   这种**双重编码**改回裸的 `0.12`（两处都写就是平方），`glow` / `ribbon` 改成 `1.0`。
   回归测试 `glow_layers_use_bare_alpha_fractions` 守这条。
   （同一改动顺带修正了按键弹窗的边框透明度：`alpha: border_alpha` 之前被忽略、边框恒为
   不透明，现在等于 Python `overlay.py:167` 的 `180 * cls.opacity * item.opacity`。）

### 验收（1920×1080；关悬浮标/点击波纹，density=10、opacity=100%）

判据：屏幕下缘轨迹带内的高饱和彩色像素数 − 关拖尾时同带基线。

| 特效 | 补丁 008 之前 | 修复后（净增） |
|---|---|---|
| 像素尘埃 pixie_dust | 全屏无输出 | **+5156** |
| 粒子光环 circle_particles | 全屏无输出 | **+1244** |
| 蛛丝缠网 spider_web | 全屏无输出 | **+341** |
| 辉光圆影 glow | 全屏无输出 | **+854** |
| 飘带 ribbon | 全屏无输出 | **+6419** |

（"之前"是 `UpdateLayeredWindow` 首帧后即失败的状态：五个特效一个不剩。带内基线
7177 px 是桌面自身内容，净值才是特效。）五个特效均已逐张截图目视确认。

蛛丝缠网偏弱是**原版设计**：Python `spider_web.py` 同样是
`anchor_count = max(10, density*5)` 的锚点撒满整屏、只连 250px 内的锚点，单屏通常只连
上几条；调高「粒子密度」可加锚点。Rust 侧逐行对齐，未做偏离原版的"增强"。

门禁：fmt ✅ / **237 tests** ✅ / clippy `-D warnings` exit 0 / release build ✅（exe 2.94 MB）。

### 工具

`tools/fx_verify.py`（新增）：逐个特效开关两次截图，按下缘轨迹带的高饱和像素数判定
"到底画没画出来"。两个坑写进了模块 docstring：

- **不能用"开关两次截图做差分"**：桌面上有正在播放的网页视频、会动的播放进度条和任务栏，
  同一位置隔 5 秒的两张全屏截图在带内能有 **11 万**像素不同，五个特效的差分数全挤在 11 万
  上下、彼此差不到 2 倍，信号被桌面自身的动画淹没。彩色计数（高 chroma）对灰阶动画免疫。
- **开跑前要把鼠标归位静置 ~2.6s**：「飘带」按鼠标历史成形，会从启动时的鼠标位置（可能
  在屏中央）向路径起点拉出一大片扇形、落到测量带之外，上次就因此把它误判成 0。

---

## 补丁 009: 整块清除快路（清屏 14ms → 0.5ms）

### 问题（用户实测：拖尾一开就卡）

像素尘埃（pixie_dust）开启后整机明显卡顿，而原版 Python（`.bf/0702-1529 1.0.0 正式版`）
同样特效却流畅。逐段计时定位到覆盖层**单帧 20.7ms**，其中：

| 分段 | 耗时 | 占比 |
|---|---|---|
| 清屏 `clear_rect(全屏)` | **14.16ms** | 68% |
| 帧推进 `frame()+render()` | 0.03ms | 0% |
| 画 `translate_offset` | 1.43ms | 7% |
| present（见补丁 013） | 4.3ms | 21% |

### 根因

`Canvas::clear_rect` 在 skia 后端走的是 `fill_rect → fill_round_rect → fill_path`，即
**通用路径光栅器**：一个轴对齐的整数矩形也要走扫描线转换 + 逐像素 alpha 混合，实测
1920×1080 清一次 **12–17ms**（约 7ns/像素）。而覆盖层**每帧**都要清一次全屏。

### 修复（windui 侧，`render/skia.rs`）

`BlendMode::Clear` 的语义是「不看目标内容，结果恒为 0」，所以只要矩形落在**物理整数
像素**上，直接按行 memset 与光栅器的结果**逐像素相同**。快路条件（四者同时成立才走）：

- `radius == 0.0`（真矩形，无圆角覆盖率）
- 无渐变
- `blend == BlendMode::Clear`
- 裁剪栈 / 离屏层均为空（前者要逐像素乘 mask，后者目标是层缓冲）

新增 `device_rect()` 把逻辑矩形换成物理整数矩形（`align_to_device` 已保证整数，非整数
则返回 `None` 回退原路径），新增自由函数 `clear_device_rect()` 按行 `fill(0)`。

### 验收

清屏 **14.16ms → 0.56ms**；单帧 **15.63ms → 1.9ms**。

---

## 补丁 010: 唤醒不再强推「正在按帧驱动」的窗口

### 问题

补丁 006 的输入门只把唤醒范围收窄到**唯一**消费窗口，但那个窗口（CurSeen 里是整屏覆盖层）
依旧被**每条输入事件**各推一次 `InvalidateRect(None)`。而 `InvalidateRect` 引发的 `WM_PAINT`
**不受帧率配速约束**——消息泵一空就合成一条。实测拖尾开启时：

- 唤醒 **1000 次/秒**（= 鼠标移动事件速率）
- 覆盖层跟着画 **44 帧/秒 × 20.7ms ≈ 93% 单核**
- 且每帧都是**整窗**（`None` 参数抹掉了 `pending_damage` 的脏区收窄）

帧画得越便宜、帧数就越多，CPU 照样吃满——这是一条**没有配速上限**的回路。

### 修复（windui 侧，`platform/win32/mod.rs`）

`WM_APP_WAKE` 里对每个窗口追加判据 `!wants_animation()`：已经在被帧循环按帧驱动的窗口
**不需要这一推**——消息泵的动画分支本来就会在显示刷新率内给它出帧，且用的是**脏区**。
唤醒在这条路径上只该负责"让通道被排空"，而排空恰好发生在那一帧的渲染里。不在按帧驱动
（空闲、静态界面）的窗口才保留这一脚，否则通道消息要等到用户下一次交互才被消费——
比如托盘点「打开控制面板」。

### 验收

唤醒仍 1000/s，但**过滤通过 0/s**；覆盖层回到帧循环的 45Hz 配速，脏区不再被抹掉。

---

## 补丁 011: 主窗构造时即认领通道消费权

### 问题

通道消费权原本是「谁先出帧谁拿到」（`sync::claim_channel_owner`，其注释假定"主窗最小，
故它天然先抢到"）。这个前提有一条漏洞：主窗**完全可能不出帧**。CurSeen 的覆盖层全关时
走休眠快路（补丁 007），`paint` 里只跑 `render_scratch` 就返回，而「打开控制面板」这个
意图消息要靠那次渲染排空——于是谁先真正出帧完全取决于**启动时序**。实测曾出现面板先出帧
抢走消费权，导致托盘「打开控制面板」点了不响应。

### 修复（windui 侧，`app/mod.rs`）

`App::run` 建出 `UiHost` 之后**立即** `crate::sync::claim_channel_owner(host.host_id)`，
再装箱。主窗就是应用替身（`App::channel` 注册在它身上），把消费权从"时序巧合"变成
"构造事实"。主窗关闭时 `release_channel_owner` 照常交还，子窗接管的老行为不受影响。

---

## 补丁 012: 输入门 — 鼠标事件不再唤醒空帧（CurSeen 侧）

### 问题

原 1.0 线有 `app::runtime::INPUT_NEEDS_FRAME` 原子门，2.0 重写时**丢了**。
代价：应用的鼠标钩子以低频钩子的**原始速率**发消息（移动约 1000 条/秒），于是一台
什么都没开的机器也要被鼠标移动钉成 ~1000 次/秒的唤醒；每次唤醒在覆盖层全关时会走一趟
空帧，实测全关机器 CPU **~15%**。

### 修复（CurSeen 侧）

- 新增 `runtime::MOUSE_INPUT_NEEDED: AtomicBool`（读端在桥接线程，故走原子）。
- 判据 `mouse_input_needed()` = `enabled_trail || enabled_draw`——比 `input_needs_frame()`
  更窄：按键显示不开帧却**完全不看鼠标**，放行它只会把 1000 条/秒的鼠标移动变成
  1000 次/秒的通道唤醒。
- 桥接线程 `spawn_input_bridge` 对鼠标事件先过门：`!MOUSE_INPUT_NEEDED → continue`。
- 写端 `sync_input_gate()` 挂三处：`Runtime::new()` 末尾（防"启动那一瞬"错放）、
  `sync_ui()`（每帧 + 250ms 对账都经过它）。挂 `sync_ui` 而非 `frame()`：三项功能全关时
  覆盖层**不出帧**，而"打开某个功能"只能从面板做——只挂 `frame()` 会撞上"打开了但桥接
  还在丢鼠标事件"的死循环。

### 验收

全关机器 CPU **~15% → 0.94%**；开着任一鼠标功能时门自动打开，特效不受影响。

---

## 补丁 013: present 合并「交换」与「拷贝」

### 问题

分层后端每次 present 分两趟：先原地 `swap_rb_inplace`（RGBA→BGRA）整张 pixmap，再逐行
`copy_from_slice` 进 DIB section。1920×1080 实测**拷 DIB 1.3ms**，且两趟都是全屏内存流量。

### 修复（windui 侧，`platform/win32/layered.rs`）

新增 `copy_swizzled(src, dst)`：一趟完成"读 RGBA 三通道换序 + 写 BGRA + 原样带过 alpha"，
直接写进 DIB。`swap_rb_inplace` 保留但标 `#[allow(dead_code)]`（不再用于主路径）。

### 验收

拷 DIB **1.3ms → 0.00ms**（合并进交换趟），present 总 **4.3ms → 3.2ms**。

---

## 卡顿根因小结（为什么 Python 原版不卡）

| 环节 | Python / Qt 原版 | Rust 重写（修复前） |
|---|---|---|
| 输入驱动 | 事件合并到刷新率，一帧只画一次 | 1000 条/秒**每条**都推一次重绘（缺补丁 012 输入门） |
| 重绘配速 | 与帧率绑定 | `InvalidateRect` 绕过帧率配速，帧越便宜帧数越多（补丁 010） |
| 清屏 | GPU 加速的矩形清除 | 全屏清除走**通用路径光栅器** 14ms（补丁 009） |
| 呈现 | 由合成器接管 | 两趟全屏内存流量（补丁 013） |
| 通道消费 | 单一事件循环串行处理 | 消费权靠"谁先出帧"竞态（补丁 011） |

五处叠加＝每帧 20ms × 44 帧/秒 ≈ 93% 单核。逐项修复后：

| 场景 | 修复前 | 修复后 |
|---|---|---|
| 拖尾开启（单线程 CPU） | **93.16%** | **33.44%**（单帧 20.7ms → 2.3ms） |
| 功能全关（整机 CPU） | ~15% | **0.94%**（空载 0.00%） |
| 清屏 | 14.16ms | 0.56ms |
| present | 4.3ms | 3.2ms |

**仍偏高的 33%** 主要落在 `present`（3.2ms，其中 `UpdateLayeredWindow` 1.45ms 是系统调用
的固有成本）与 DrawList 光栅（1.4ms），已无 10ms 级的单项，属可接受区间；进一步优化需
落到补丁 005 未做的**脏区上传**（R4）。

门禁：fmt ✅ / **238 tests** ✅ / clippy `-D warnings` exit 0 / release build ✅（exe **2.95 MB**）。
烟测：空载 CPU **0.00%**（10s 采样 0ms），私有内存 14.4 MB，工作集 33.3 MB（稳定）。

---

## 补丁 014: 覆盖层 z 序守护（置顶带 + 全屏让位）

### 问题（用户报）

开了鼠标拖尾之后，点别的软件窗口，拖尾就消失了。

### 根因（实测，不是推测）

`WS_EX_TOPMOST` 只把窗口放进"置顶带"，**不保证它在带内靠前**——系统从不替我们"守住"位置，
任何**晚于我们创建（或被抬起）的置顶窗口**都插在我们上面。而我们从不重新断言 topmost：
全仓原本一处 topmost / z-order 代码都没有。

可复现的量化证据（`tools/zorder_probe.py ontop`，在覆盖层上方放一个 600×400 的置顶窗）：

| | 覆盖层 z_rank | 该区域内拖尾墨迹 | 进程 CPU |
|---|---|---|---|
| 修复前 | 49（对方 6，被压在下面） | **0 字节**（完全看不见） | 84% |
| 修复后 | 6（对方 7，我们在上） | **112197 字节**（照常画） | 83% |

**CPU 一直很高**是关键数据：特效在**照常渲染**，只是被盖住看不见——这排除了"输入断了"，
把问题锁定在呈现层。（对照组：点一个普通窗口时墨迹比值 1.4–1.7，即普通窗口本来就压不住我们。）

### 修复（CurSeen 侧，新增 `src/platform/topmost.rs`）

`TopmostKeeper::reconcile()`，挂在已有的 250ms 对账定时器（`PANEL_POLL`）上：

1. **遮挡判据**：枚举 z 序，找"在覆盖层上方、置顶、可见、**矩形 ≥32px 且与覆盖层相交**"的
   窗口；有就 `SetWindowPos(HWND_TOPMOST)` 把自己插回置顶带。
2. **主动让位**：前台是全屏应用/游戏时 `HWND_BOTTOM` 置底，退出后自动恢复置顶。
3. **不抖动**：同一个遮挡者最多抢 2 次，之后退让并记一条日志（指名是谁），不再拉锯。

### 三条实测平台语义（都是踩出来的）

| 事实 | 数据 | 影响 |
|---|---|---|
| `SetWindowPos(HWND_TOPMOST)` 只到置顶带**底部**，反复调用**不爬升** | z_rank 48 → 6，第 2..6 次仍为 6 | 够用：真实遮挡者（后上台的普通置顶窗）也在同一层，再插一次就排到它前面 |
| 本机压在我们上面的 6 个窗口**全是 0×0 / 1×1 的系统 helper**（`ThumbnailDeviceHelperWnd`、2×`ForegroundStaging`、Quicker 面板、输入法皮肤窗、GDI+ hook 窗） | 矩形见探针输出 | **判据只看"置顶+可见"会为空转**：第一版修复就是这么失效的（对 1×1 帮手抢 2 次后退出，真正盖住我们的窗口反而没人管） |
| `SHQueryUserNotificationState` 在本机**一启动就报 `QUNS_BUSY`(=2)** | 日志明写 `SHQueryUserNotificationState=2` | 触发它的正是**我们自己**（覆盖层是前台窗、铺满整屏又无标题栏）→ "自己让自己让位"的循环。已改为只认 `QUNS_RUNNING_D3D_FULL_SCREEN` / `QUNS_PRESENTATION_MODE`，并增加"前台窗口属本进程则不算全屏应用"的豁免 |

### 验收

| 场景 | 结果 |
|---|---|
| 置顶小窗压上来 | 自动抬到它上面（rank 6 vs 7），墨迹 **0 → 112197**，日志指名 `CurSeenZOrderVictim` |
| 无边框全屏窗在前台 | `TOPMOST` 位消失（已置底让位），日志 `前台窗口铺满显示器且无边框：…` |
| 全屏窗退出 | `TOPMOST` 位恢复，日志 `全屏应用已退出，覆盖层恢复置顶` |
| 普通窗口 | 不受影响（墨迹比值 1.44–1.66） |
| 启动 | 不再误判全屏（修复前会自己把自己置底、rank 掉到 48） |

探针：`main/tools/zorder_probe.py`（`state` / `above` / `ink` / `ontop` / `e2e` / `pin` /
`yieldtest` / `elevated`）+ `main/tools/zorder_victim.py`（可造普通 / 置顶 / 全屏三种"别的窗口"）。

### 未覆盖的一路（待定）

**前台是"以管理员身份运行"的窗口**时，UIPI 会拦住低完整性进程的低级钩子（见
`platform/elevate.rs` 的说明），那段时间**根本收不到鼠标事件**——现象与"被遮挡"很像，但成因
完全不同（那个是"没画"，这个是"画了看不见"）。探针已带 `elevated` 子命令，可列出可见的提权
窗口并对其做墨迹+CPU 双测；**本次本机没有可见的提权窗口，这一路未能实测**。缓解手段是
`--elevate` 启动（已有实现，仍未实测）。判读要点：墨迹与 CPU **同时**归零才是钩子被拦，
CPU 仍然高就只是被遮挡。



---

## 补丁 015: 分层后端单缓冲 — tiny-skia 直写 DIB section

### 问题（R1 内存：常驻里最大的一块）

分层后端一直持有**两块内容完全相同的整屏缓冲**：tiny-skia 的 `Pixmap`（RGBA，渲染目标）与
`UpdateLayeredWindow` 要的 GDI DIB section（BGRA，呈现源），每帧再做一次跨缓冲的 RGBA→BGRA
拷贝（补丁 013 已把「换序 + 拷贝」合并成一趟，但两块缓冲和那次整屏内存流仍在）。1080p 下
白占 7.9 MB，4K 下 33 MB——是 CurSeen 活跃态私有内存里最大的单块。

### 修复（windui 侧）

- **`render::DibTarget`（新）**：`RenderTarget` 的「直接画进调用方内存」实现——
  `tiny_skia::PixmapMut::from_bytes`（无对齐要求）把画布指到任意一段 32bpp 内存。
- **`SkiaCanvas` 泛化**：后备缓冲从 `&mut Pixmap` 改为 `PixmapMut<'a>`（新增
  `IntoPixmapMut` 桥接 trait，`SkiaCanvas::new(&mut pm)` 四十余处调用点零改动）；
  `TextEngine::draw` 同步收 `&mut PixmapMut`（CurSeen 侧是唯一消费者）。
- **`layered.rs` 单缓冲重写**：DIB section 成为**唯一**后备缓冲；呈现顺序改为
  「tiny-skia 直写 DIB → 就地交换 R/B（`swizzle_rb_inplace`）→ `UpdateLayeredWindow`」。
  跨缓冲整屏拷贝消失；补丁 013 的 `copy_swizzled` 退役。
- **为什么"就地交换"安全**：交换后缓冲变成 BGRA，下一帧 tiny-skia 会把它当 RGBA 读
  （混合要读目标）。前提是宿主每帧**先整窗 clear 再全量重画**——CurSeen 覆盖层正是如此
  （`OverlayCanvas::paint` 首行 `clear_rect` 整窗）。若将来接脏区局部重绘，此前提不成立，
  届时按 `win32/mod.rs` 里 `SkiaBackend` 的 `swap_rb_rect`（按矩形翻）同款处理。

### 验收（1920×1080，60Hz）

| 项 | 修复前 | 修复后 |
|---|---|---|
| 活跃态整屏私有块 | 2 × 7.91 MB（Pixmap + DIB） | **1 × 7.91 MB** |
| 面板关闭后 Private | — | **12.30 MB**（活跃拖尾）；全关休眠 **10.30 MB** |
| 空载（全关休眠 30s 采样） | — | **0.000% 单核（0 tick）**，缓冲已释放（无 7.9 MB 块） |
| 五特效可见性（带内净增） | — | pixie_dust +72617 / circle_particles +38444 / glow +8006 / ribbon +6533；spider_web 带内 +0 但**全屏净增 +9361**（锚点撒满整屏、连线多在带外——原版设计即弱，非回归，颜色正确） |
| z 序守护（补丁 014）无回归 | — | 置顶 victim 压上 → rank 6 vs 7，墨迹 **114803**，拖尾画在它上面 |
| PARKED 过渡帧 | — | 拖尾开着 → 面板关闭：带内墨迹 10267 → 3719（回到桌面基线，**无残影**），7.9 MB 块 1 → 0 |
| 活跃渲染单位成本 | — | 与 013 时期一致（~35µs/图元；密度 1 轻负载 1.0–1.5ms/帧 vs 密度 5 高速输入 14–49ms/帧，纯负载缩放，无回归） |

工具：`WINDUI_PROF=1` 环境变量可打印每帧绘制拆分（`render/prof.rs`，本次用于排除回归）。

### 门禁

windui：fmt ✓ / clippy `-D warnings` ✓ / **749 + 70 tests** ✓（含修复 5 处测试旧签名
`&mut Pixmap → &mut PixmapMut`、3 处 `WindowState` 补 `click_through` 字段、1 处
`map_or → is_none_or` 新 lint）。main：fmt ✓ / clippy ✓ / **240 tests** ✓ / 依赖门禁 ✓。
release 重建 **3,090,944 B**，哈希与产物一致，烟测通过。

---

## 补丁 016: 托盘菜单的前台代理窗口（不让整屏覆盖层接焦点）

### 问题

`track_menu` 把**主窗口**当菜单 owner。`TrackPopupMenu` 要求 owner 先被
`SetForegroundWindow` 激活（否则"点击菜单外部不消失"失效），而 CurSeen 的主窗是整屏、
置顶、鼠标穿透的覆盖层——焦点就此落进一个用户看不见也点不到的窗口，且全部菜单消息都要
过那个整屏 layered 窗口的窗口过程。

### 修复（windui 侧，`.3rdParty`）

- 新增菜单**前台代理窗**：1×1、屏幕外（-100,-100）、`WS_EX_TOOLWINDOW`（不进任务栏与
  Alt+Tab），但**必须可见**——不可见窗口无法被激活，菜单就不响应"点击外部关闭"。
- 菜单关闭后**把焦点还给**弹出前的前台窗口（`prev_fg`，先 `IsWindow` 确认还活着）。
- 计时落日志：`托盘菜单：前置 X ms，存续 Y ms`，用于分辨"卡在前置还是卡在菜单期间"。
  （补丁 017 起这个代理窗改在菜单线程上建，见下。）

### 验收

`build/log/curseen-20260915.log`：`前置 4.0 / 6.7 / 5.9 / 4.0 ms`，`存续 539–2286 ms`。
**前置稳定在个位数毫秒**——这条数据排除掉了 `SetForegroundWindow` 同步阻塞这个假设，
把 017 的方向锁定在"模态循环本身"上。

---

## 补丁 017: 托盘菜单搬出 UI 线程（右键自身托盘卡顿，CPU/内存不升）

### 问题

用户报告：对系统托盘图标右键"非常卡顿"，但**只对 CurSeen 自己的图标**，别家图标正常；
任务管理器里 CPU 与内存**都没有变化**。

根因：右键走 `on_tray_message` → **在窗口过程内部**同步 `TrackPopupMenu`。它是模态的，
自带消息循环，一直跑到菜单关闭才返回。日志里"存续"普遍 1.6–2.3 秒，这段时间 UI 线程的
消息泵被别人的循环接管：拖尾、面板、z 序守护全都只能靠它**转手**的消息活着。更糟的是它
从窗口过程里发起，shell 转发托盘消息时若同步等这个窗口过程返回，explorer 的任务栏会一起
被挂住——于是"整个系统都变卡"。

三条现象同时对上：**CPU/内存不升**（没有人在计算，只是空等）、**只卡自己的图标**（别家
菜单在别家进程里）、**补丁 016 证明前置只花几毫秒**（卡不在 `SetForegroundWindow`）。

### 修复（windui 侧）

- **菜单在独立线程弹出**（`show_tray_menu`）：UI 线程只负责建菜单（`build_menu`，要读
  `Signal` 勾选态，必须在 UI 线程）→ 把 `PopupMenu` 交给菜单线程 → **立刻返回**。
- **owner 建在菜单线程**：菜单消息投递给 owner，owner 属于别的线程时系统要跨线程同步，
  那等于把等待又塞回 UI 线程。故新线程自建 1×1 代理窗（补丁 016 的形状），并为此单开
  窗口类 `WindUiMenuOwnerClass`（只 `DefWindowProcW`，不把 `AppHost` 带进别的线程）。
- **结果经消息回 UI 线程**：新增 `tray::WM_TRAY_RESULT`（WM_APP+3），菜单线程
  `PostMessageW` 回宿主，由 `app_host_proc` 那条分支跑 `run_item` + `run_tray_actions`
  ——回调最终要动窗口，而窗口只属于创建它的线程。
- **同刻只允许一个菜单**：`MENU_SHOWING` 标志位，菜单期间的重复右键直接丢弃；线程启动
  失败时**必须复位**，否则此后托盘菜单永远弹不出来（用户只会看到"右键没反应"）。
- 跨线程的两处 `Send`：`PopupMenu`（`HMENU` 是进程级 USER 对象、不绑定线程）显式补
  `unsafe impl Send`；`HWND` 以 `isize` 过线程边界再还原。

### 验收判据（用户实测）

1. 右键 CurSeen 托盘图标 → 菜单弹出期间，**鼠标在菜单外移动时拖尾应当照常跟随**
   （修复前这 2 秒是冻结的——这是最直观的判据）。
2. 菜单关闭后焦点回到原来的窗口，第一次点击不被"激活"吃掉。
3. 日志仍打 `托盘菜单：前置 X ms，存续 Y ms`；`存续` 照旧（用户看菜单的时间），但 UI
   线程不再被它占用。

### 门禁

windui：fmt ✓ / clippy `--all-targets --release -D warnings` ✓。main：clippy ✓ /
**231 tests** ✓（`cargo test --release`）。

## 补丁 018: 子窗指定初始位置（`Window::position`）

### 问题

屏幕标注的**文字工具**要开一个小输入窗，落点应当跟着用户刚点的那一下走。但 `Window`
只有 `centered`：`Window::new(...).centered(true)` 在设了 `owned` 之后含义是"居中在
发起窗口上"，而我们的发起窗口（覆盖层）铺满**整块虚拟桌面**——双屏合计 3840 宽时，
居中落在左屏中间，用户在右屏点"写字"，输入框从另一块屏上冒出来。

`WindowConfig` 里其实早有 `pos: Option<(i32, i32)>`（补丁 003 给主窗加的，建窗函数
`create_window` 一直在用），缺的只是通路：`WindowRequest` 不带这个字段，
`Window` 也没有对应的构建方法，于是子窗永远拿不到非默认位置。

### 修复（windui 侧）

- `WindowRequest` 增 `pos: Option<(i32, i32)>`（同 `centered` 的语义：同时设置以本项为准）。
- `Window::position(x, y)` 构建方法，`Window::new` 的初值为 `None`（行为不变）。
- `App::open_pending_windows` 把请求转成 `WindowConfig` 时**显式**写 `pos: req.pos`。
  这一行不能省：那里用的是 `..WindowConfig::default()` 兜底，漏掉的字段会被静默填成
  默认值，症状是"设了位置但窗口还是出现在别处"。

不做坐标钳位：哪个显示器、允许溢出多少是应用才知道的事（本次调用方自己钳）。

### 验收判据

1. `Window::new(..).position(x, y).content(..)` 打开的子窗，左上角落在 `(x, y)`
   （物理像素，虚拟桌面坐标，可为负）。
2. 不调 `position` 的子窗行为与补丁前完全一致（`pos = None` → `CW_USEDEFAULT` 或
   `centered` 那条路）。

### 门禁

windui：fmt ✓ / clippy `--all-targets -D warnings` ✓。main：fmt ✓ / clippy
`--all-targets --all-features -D warnings` ✓ / **319 tests** ✓。

---

## 补丁 019 — 屏幕标注收尾：线型生效 + 矩形/椭圆改描边 + UIPI-free 离屏验收

### 背景
屏幕标注（白板/屏幕两种模式）的功能代码在补丁 017/018 已落地，但有两处"画出来和预期不符"、
加上 UIPI 让端到端验证走不通，本次一并收口。用户口径："聚光灯/放大镜属于别的屏幕工具，
以后再加；屏幕书写/标注按你觉得最好的方式来，要小而美、性能要保住。"

### 问题 1：面板「线型」下拉没效果
`config.drawing.pens[i].style` 会被面板写入并一路带到 `Pen.style`，但
`controller.rs::draw_stroke` 从不读它——用户选「虚线」画出来的还是实线。Python 1.0.0 用
Qt `DashLine/DotLine/DashDotLine` 实现过，Rust 移植时漏了。

### 修复 1（算法层，不扩契约、不打 windui 补丁）
- 新增 `modules/drawing/dash.rs`：按 `PenStyle` 把一条折线切成若干段**独立**子折线（几何展开），
  `DrawCmd::Polyline` 契约完全不变；windui 的 `draw_polyline` 本来就能被多次调用。
- 实线 / 点数 < 2 / 线宽 ≤ 0 / NaN 线宽 / 全重复点 → 返回**原样一段**（不退化、不 panic、不死循环），
  调用方对实线零分支。
- pattern 按线宽同比放大（`Dashed=16u/8u`、`Dotted=4u/6u`、`DashDot=16u/6u/4u/6u`，单位=线宽倍率）。
- `draw_stroke` 在**历史笔迹**与**进行中 live stroke** 两处都接上 `dash::segments`。

### 问题 2：矩形/椭圆被填实
翻译层 `runtime/render.rs` 对 `Shape::Rect` 走 `fill_round_rect`、对 `Shape::Ellipse` 走
`fill_ellipse`，把整个图元填成色块——与工具栏里的空心方框/空心椭圆图标自相矛盾，
"拖矩形工具出来一个实心色块"。

### 修复 2（翻译层）
- `Shape::Rect` 改用 `stroke_round_rect`（线宽随 DPI 缩放，`radius = max(width*0.5, 配置圆角)`）。
- `Shape::Ellipse` 改为描边：外弧正向 + 内弧反向拼成一条**简单**多边形用 `fill_polygon` 填，
  内径退化时回落实心圆（`fill_circle`）。闭合由构造保证（首尾角度都是 0），无端帽/接头/缺角。
- 圆环放大缺角一并修掉：旧实现 32 段固定折线近似 + 描边，半径放大后弦线内切肉眼见棱角；
  现用自适应细分的环带多边形压到亚像素以下。

### 问题 3：UIPI 让端到端验证不可能
应用启动即提权（`relaunch_elevated`），非提权探针注入的 `SendInput`/鼠标被 UIPI 静默丢弃
（`SendMessage` 实测 `err=5`），"启动 exe → 注入 Alt+G → 拖一笔 → 截图比对"永远失败，
且失败样子和"功能坏了"一模一样。

### 修复 3（离屏验收探针，绕开整条窗口/输入链路）
- 新增 `examples/render_probe.rs`：驱动**真实** `DrawingController` 跑完一整场标注会话 → 产出
  **真实** `DrawList` → 经**生产用的同一个** `translate_offset` 渲进 `windui::DibTarget` 的
  RGBA 缓冲 → 合成到一张合成的"桌面"上存 PNG。像素判据在进程内直接算，不依赖任何外部工具。
- 验的是"算法层 → 像素"这一段（标注功能自己的全部代码），不验"Windows 把窗口画到屏幕"
  （那是 windui 职责，另有面板探针覆盖）。
- 判据：①工具栏外框预测 vs 实测偏差 ≤1px；②当前工具高亮底色 > 300px；③黑/绿/红三笔都着墨；
  ④实线扫出 1 段、虚线扫出 ≥15 段；⑤矩形/椭圆**边框**着墨、**内部 0%**；⑥退出后工具栏
  0 条描边指令；⑦整屏 1080p 一帧 ≈4.4ms（最坏 60fps 连续重绘 ≈26% 单核；空闲 0%）。

### 验收判据
1. 选「虚线」画笔拖出的笔迹是断的（探针实测绿虚线 27 段、黑实线 1 段），不再与实线无差别。
2. 矩形/椭圆工具拖出空心描边（内部 0/10944 px 着墨），与工具栏图标一致。
3. 工具栏几何与 `toolbar::layout` 预测完全一致（偏差 0px）。
4. 退出标注后工具栏整体消失（DrawList 不再含其 40+ 条指令），否则一块点不动的工具栏常驻屏上。
5. 性能：整屏 1080p 重绘 ≈4.4ms/帧；静止时 `frame_demand` 报 `Idle`（见补丁内回归测试）。

### 门禁
main：fmt ✓ / clippy `--all-targets --all-features -D warnings` ✓ / **337 tests** ✓ /
`build --release` ✓ / `examples/render_probe` 全部 21 项判据通过 ✓。
部署：`build/curseen2.exe`（3,156,992 B，静态链接 crt）。

## 补丁 020: 屏幕书写设置页两修（标题对齐 + 热键捕获/激活）

### 问题 1：带开关的卡片标题文字右移 2px
`card.rs::build_title_row` 开关列：`checkbox("", sig)` 空标签 `measure` 返回
`BOX_SIZE+GAP+0 = 26px`，无开关卡片的占位 `leaf().width(SWITCH_W=24)` 为 24px → 差 2px，
带开关的「画笔设置」标题比无开关的「热键设置」右移 2px。

### 修复 1
`checkbox` 分支加 `.width(SWITCH_W)`：布局引擎对显式 `.width()` 取 `Dimension::Exact`，
精确报告 24，与占位对齐到同一 x。

### 问题 2a：快捷键开启屏幕标注/白板没反应
`overlay.rs::UiMsg::Input` 仅当 `input_needs_frame()` 为真才 `request_repaint()`；总开关
`drawing.enabled` 默认关 → 覆盖层不出帧 → `frame()` 永不跑 → `input_queue` 永不消费 →
按 Alt+G 无任何反应。`handle_key` 又把两个 toggle 关在 `!drawing.enabled` 守卫里。

### 修复 2a（激活豁免 + 踢帧）
- `drawing/mod.rs::handle_key`：`input_suspended` 单独判（挂起仍挡），ToggleScreen/ToggleWhiteboard
  从 `!drawing.enabled` 守卫**豁免**——总开关关着也能靠 Alt+G 叫起标注（对齐 Python 1.0.0
  无总开关语义）。
- `runtime/mod.rs`：`input_needs_frame()` / `mouse_input_needed()` 增
  `|| drawing.controller().is_drawing()`，进模式后继续出帧。
- `overlay.rs::UiMsg::Input`：收到 ToggleScreen/ToggleWhiteboard 热键时**额外踢帧**
  （`match_drawing` 不读 `enabled`），让 `frame()` 跑起来消费该键。
- 测试 `screen_hotkey_does_nothing_when_feature_off` 改名 `screen_hotkey_starts_feature_when_switch_off`
  并断言「关着也能进书写模式」（旧断言即本次要修的 bug）。

### 问题 2b：热键无法用按键录入
windui 无 per-element `on_key`；原 `text_input` 只能手敲字符串，按快捷键录不进去。

### 修复 2b（点击捕获框）
- `ui/model.rs`：新增 `HotkeyField` 枚举（11 项）+ `PanelModel::hotkey_capture:
  Signal<Option<HotkeyField>>`（纯 UI 态，不落盘）+ `hotkey_signal(field)` 映射到对应 `hk_*` 信号。
- `ui/pages/drawing.rs::hotkey_row`：`text_input` 换成可点击捕获框（`label_signal` + `border`
  + `bg`），`on_click` 清空该信号并置 `hotkey_capture=Some(field)` 进入捕获态（显示「按下按键…」）。
- `core/hotkeys.rs`：新增 `pretty_hotkey()` 把 `pressed` 的全大串（`ALT+G`）转人读格式
  （`Alt+G`，修饰键排前）。
- `runtime/mod.rs`：新增 `try_capture_hotkey()`，在 `frame()` 输入队列循环里、交给 `drawing.tick`
  之前拦截 `KeyDown`：修饰键-only 保持捕获态继续等；否则 `pressed` + `pretty_hotkey` 写回
  `hotkey_signal(field)` 并清零捕获。捕获态下 `overlay.rs` 已在 `UiMsg::Input` 踢帧保证
  `frame()` 跑得到。
- 捕获结果经 `hk_*` 信号 → `apply_to` 实时 diff 回配置，**自动落盘**（无需新增持久化路径）。

### 验收判据
1. 标题对齐：「画笔设置」（有开关）与「热键设置」（无开关）标题文字左边缘同 x（checkbox 钉 24px）。
2. 激活：总开关关着按 Alt+G → 进入屏幕标注模式（新测试 `screen_hotkey_starts_feature_when_switch_off`）。
3. 捕获：点热键框 → 显「按下按键…」→ 按 Ctrl+Shift+D → 框显示 `Ctrl+Shift+D` 且配置落盘；
   按修饰键-only 不结束捕获。
4. 门禁：fmt ✓ / clippy `--all-targets --all-features -D warnings` ✓ / **337 tests** ✓ /
   `build --release` ✓。部署 `build/curseen2.exe`（3,159,552 B，静态链接 crt）。

## 补丁 021：快捷键激活在「面板开着」时仍无反应（挂起豁免 toggle）
- 用户实测 Alt+G 仍无反应。真根因：应用启动即打开控制面板 → `input_suspended()=true`，而 `DrawingModule::handle_key`/`tick` 在挂起时**整段 return**——上一轮（补丁020）只豁免了 `!enabled`，漏了 `input_suspended`。
- 修复（`modules/drawing/mod.rs`）：把 ToggleScreen/ToggleWhiteboard 匹配提到 `input_suspended` 守卫**之前**（全局热键语义：面板/文字窗开着也生效）；挂起守卫移到 toggle 之后继续挡其余键；`tick` 挂起分支对非 repeated KeyDown 仍调 `handle_key`（只有 toggle 能穿透）。
- 测试：新增 `hotkey_toggles_even_when_input_suspended`（挂起态按 Alt+G 进入书写）；旧 `suspended_hotkeys_do_nothing` 改名 `suspended_tool_hotkeys_blocked`（挂起态画笔切换仍被挡）。
- 门禁：fmt✓ clippy -D warnings✓ 338 tests✓ build --release✓；部署 build/curseen2.exe（3,160,064 B）。
- 排查过程中顺带确认：windui park 态 `request_repaint` 不触发 WM_PAINT，但 `WM_APP_WAKE` 通道消息对 `wants_wake_repaint()` 窗口会 InvalidateRect，故 overlay 侧踢帧链路成立。

## 补丁 022：无边框 / 不可缩放窗的外圈 8px 被当成缩放热区（截图+贴图的前置修复）

### 背景
本轮要做两个**无边框全屏级窗口**：截图区域选择窗（`layer::region`）与贴图窗（`layer::pin`）。
两者都要求"窗口内任意一点都能起手"——前者要能从最边上开始框选，后者要能整窗拖动、整窗双击。

### 问题：最外一圈指针事件到不了客户区
`platform/win32/mod.rs::handle_nchittest`（`:2265`）的实现**只看指针到边缘的距离**：
命中窗口边缘 `RESIZE_BORDER_LOGICAL = 8` 逻辑像素就返回 `HTLEFT/HTTOP/HTRIGHT/...`。
它**不看窗口样式位**，于是对所有窗口一律生效——而 `WM_NCHITTEST` 一旦返回 HTLEFT 之类的
命中码，那条 `WM_LBUTTONDOWN` 就被系统拿去做缩放，**永远不会派发到客户区**。

实测代价（两个消费方各一半）：
- 区域选择窗：最外 8px 点不出选区起手，用户以为"贴边点不动"；
- 贴图窗：四周 8px 死区吞掉 `WM_LBUTTONDOWN`，`PointerEvent::click_count` 拿不到双击；
  60px 的小贴图窗有近半面积落在死区里，等于双击取消在最边上失效。
- 顺带：控制面板本身是 `resizable(false)`，它的外圈 8px 也在这个热区里（一直如此，只是没人量过）。

### 修复 022-a（`platform/win32/mod.rs::handle_nchittest`）
`WS_THICKFRAME` 是"这个窗能缩放吗"的**唯一权威判据**：`resizable(false)` 的窗口本来的样式就是
`WS_OVERLAPPEDWINDOW & !WS_THICKFRAME`，分层覆盖窗是 `WS_POPUP`，两者都不该缩放。

```rust
let can_resize = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 } & WS_THICKFRAME.0 != 0;
if !can_resize {
    // 仍然尊重宿主声明的拖动区（Element::window_drag），其余一律还给客户区。
    let drag = state_from(hwnd).map(|s| s.handler.window_drag_at(...)).unwrap_or(false);
    return LRESULT(if drag { HTCAPTION as isize } else { HTCLIENT as isize });
}
```

比计划书里"直接 `return HTCLIENT`"多留了一步 `window_drag_at`：把外圈还给客户区是对的，
但**别把宿主自己声明的拖动区一起吞掉**（那是另一个特性，与缩放无关）。

### 未做（可选，仍留在计划书里）
- **022-b** `.window_topmost(bool)`：本轮用"owner 是 topmost + `activate_window_by_title` 补刀"
  撑住置顶，够用；直通 `WS_EX_TOPMOST` 只是让语义更直白。
- **022-c** `create_window` 文档补一节"`Window::position` 物理像素 / `Window::new(w,h)` 逻辑 dp"。
  这条**已经踩过**（本轮修的就是它），但修在调用侧（`WorldMap::logical_size()`），文档还没补。

### 验收判据
1. 区域选择窗最外 8px 能起手框选（真机手工，§11.3-4）；
2. 贴图窗（含 60px 小图）四边任意点可拖动、可双击取消（真机手工，§11.3-11/12）；
3. 面板右边缘像素回归：补丁把面板外圈 8px 从"缩放热区"变成客户区，滚动条避让常量
   `WINDOW_EDGE_INSET` 是按"有热区"设的——需用 `curseen-panel-pixel-verify` 的探针重量一次
   右侧留白（**待做**，见实现记录的"未验收项"）；
4. `cargo test` 全绿（本改动不影响任何单测路径）。

### 门禁
fmt ✓ / clippy ✓（新代码零告警）/ **404 + 4 + 1 tests** ✓ / `build --release` ✓；
部署 `build/curseen2.exe`（3,279,360 B，md5 与 `temp/target/release/` 逐字节一致）。

## 补丁 023：直角 + 1:1 + 无裁剪的图像 blit 不再构造整屏 Mask

### 背景
截图/贴图/屏幕注释统一改造（原计划书 `main/docs/snap-pin-plan-20260916.md`，**已不在
仓库**，见文首）把"冻结屏幕"做成了一条 `DrawCmd::Image`——**每帧**都要把一张整屏位图
铺到覆盖层上。计划书 §4.2 给这条路径
定的预算阈值是 **15 ms/帧**（超了就走 B 路线：底图独立窗只画一次），并明确要求"实测，
不能只靠估计"。
      ★ 2026-09-27 核查：该阈值与实施后的实测值都在 `tests/probes/freeze_overlay_poc.rs`
      的用例注释里，**那里才是现在的唯一出处**。

### 问题：一次 blit 被做成了三趟整屏操作
`render/skia.rs::SkiaCanvas::draw_image` 无条件构造 mask：

```rust
let Some(mut mask) = Mask::new(mw, mh) else { return };        // ① 整屏分配 + 清零（1080p = 2 MB）
mask.fill_path(&path, FillReason::Winding, true, Transform::identity());  // ② 把目标矩形填进 mask（整屏抗锯齿路径填充）
...
t.draw_pixmap(0, 0, img_ref.as_ref(), &paint, transform, Some(&mask));    // ③ 带 mask 采样混合
```

而 mask 在"直角 + 1:1 + 无裁剪栈"这条**最常见**的情形下恒等于目标矩形本身——三趟里有两趟
纯属白做。实测（1080p 整屏冻结帧，release 出货配置，5 批 × 20 帧取最小）：

| 路径 | 每帧 |
|---|---|
| 空 DrawList（基线） | 0.00 ms |
| 整屏白 Fill（白板那条既有路径） | 0.40 ms |
| 整屏 Image（补丁前） | **18.7 ms** |

### 修复（`render/skia.rs::draw_image`）
在构造 mask 之前加一条守卫分支，四个条件全部成立才走快路并直接 `return`：

- `radius <= 0.0` —— 没有圆角要收口，mask 退化成矩形；
- `sx == 1.0 && sy == 1.0` —— 上一步的 1:1 吸附已把缩放归一到 1，落点 `tx/ty` 也取过整；
- `self.clips.is_empty()` —— 不必与视口求交；
- `self.layers.is_empty()` —— 目标就是主缓冲，不是离屏层。

等价性论证（写进代码注释）：直角 ⇒ 圆角 mask 就是矩形；1:1 且整数落点 ⇒ 每个目标像素的
采样点恰好落在源纹素正中心，`Nearest` 取到的就是那颗纹素，与 `Bilinear` **逐像素相同**；
无裁剪栈 ⇒ mask 只剩"把 dst 之外判掉"的作用，而 `draw_pixmap` 本来就把源图限制在它自己
`iw×ih` 的范围内。所以快路与慢路结果**逐像素一致**。

这条分支对全应用生效（叠层底图、贴图窗大图、悬浮标动画帧、面板图标——全都是直角 1:1）。

### 实测结果
| 配置 | 每帧 | exe 体积 |
|---|---|---|
| 补丁前 | 18.7 ms | 3.28 MB |
| 补丁后 | **16.0 ms** | 3.28 MB |
| 补丁后 + 渲染内核单包 `opt-level=3` | **16.0 ms** | 3.56 MB |
| 补丁后 + 全局 `opt-level=3`（对照） | 11.8 ms | 4.57 MB |

（另见 `main/Cargo.toml` 里 `[profile.release.package.*]` 那段：只把渲染内核提到 `opt-level=3`
是刻意的取舍，全局提速会让 exe 从 3.56 MB 涨到 4.57 MB。表格与"为什么仍差 1 ms"的完整
账就记在同一份用例注释里（`tests/probes/freeze_overlay_poc.rs`）；
★ 原先指向的 `main/docs/snap-pin-implemented-20260916.md` **已不在仓库**，按它去找一定扑空。）

### 验收判据
1. **像素不变**：`tests/freeze_overlay_poc.rs` 的冻结帧 1:1 抽样断言（9/9 与原抓屏一致、±2/通道）
   在补丁前后都必须通过——这条快路只允许"更省"，不允许"画得不一样"；
2. 面板图标 / 悬浮标 GIF / 贴图窗小图这些同样走 `draw_image` 的地方目视无变化（真机手工，§11.3）；
3. 带裁剪的滚动视口（控制面板）不受影响：`self.clips` 非空即回退原路径。

### 门禁
fmt ✓ / clippy ✓（新代码零告警）/ **404 + 4 + 2 tests** ✓ / `build --release` ✓；
部署 `build/curseen2.exe`（3,560,960 B，md5 `d324a21aca5d0ddf6b6d9d774ef5f7be`，与
`temp/target/release/` 逐字节一致）。运行期配置驱动验收 2 轮 6 项全 PASS
（`main/tools/smoke_snap_pin.py` → `temp/smoke_snap_pin.txt`）。

---

## 补丁 024：无边框/分层窗口的外框尺寸不再按"带框窗口"反算；无边框窗口一律直角

### 背景
用户报"贴图有问题，右边和下方有黑边（文字贴图也一样）"，以及"贴图必须是直角的，当前是圆角"。
两件事都出在同一个地方——`create_window` 对 `frameless` 窗口的两处处理。

### 问题一：客户区比请求尺寸大一圈（黑边）
`create_window` 建窗时这样算外框（改前）：

```rust
let mut win_style = if cfg.resizable { WS_OVERLAPPEDWINDOW } else { WS_OVERLAPPEDWINDOW & !(...) };
if owner.is_some() { win_style &= !WS_MINIMIZEBOX; }
let (phys_w, phys_h) = frame_size_for_client(cfg.width, cfg.height, init_scale, sys_dpi, win_style);
```

而**无边框窗口的非客户区是被 `WM_NCCALCSIZE` 消掉的**（`handle_nccalcsize` 在非最大化时
不动 `rgrc[0]` ⇒ 客户区 = 整窗）。于是：

```
请求客户区 400×300（逻辑）
  → frame_size_for_client 按 WS_OVERLAPPEDWINDOW 加进"标题栏 + 边框" → 外框 402×(300+31)
  → 建窗 402×(300+31)
  → WM_NCCALCSIZE 让客户区铺满整窗 → 客户区 402×(300+31)   ← 比请求的大一圈
  → 布局按 402×(300+31) 排，而贴图按 400×300 画
  → 右下多出 (2, 31) 的死区，窗口底色透出来 = 用户看到的"黑边"（下方明显、右侧细）
```

同一个错误在 `WM_GETMINMAXINFO` 分支里**不存在**——那里本来就写了
`if state.frameless || is_layered { w*scale, h*scale }`。补丁 024 是把建窗路径对齐到那条
既有规则上，顺手抽成可测函数 `outer_size_for_client()`。

### 问题二：DWM 圆角啃掉贴图四角
`if cfg.frameless` 分支里无条件 `DwmSetWindowAttribute(DWMWCP_ROUND)`。走 `.frameless(true)`
的全是"内容就是窗口本身"的窗口（全屏覆盖层、截图选区窗、贴图窗），系统圆角会把贴图四角
啃掉一块露出桌面。改为 `DWMWCP_DONOTROUND`——带框窗口（控制面板）不走这个分支，仍是系统
默认圆角，观感不受影响。

### 修复（`platform/win32/mod.rs`）
1. 新增 `fn outer_size_for_client(frameless_like, w, h, scale, dpi, style)`：`frameless_like`
   （`cfg.frameless || cfg.layered`）直接 `round(w*scale)`，否则才走 `AdjustWindowRectExForDpi`；
2. `DWMWCP_ROUND` → `DWMWCP_DONOTROUND`（仅 `frameless` 分支）；
3. 顺带（为下游可测）：`DispatchResult` 增 `pub capture: Option<Option<NodeId>>`，在
   `run_detached` 与 `dispatch_pointer` 里如实上报"本次分发动没动捕获"。此前它只存在于
   私有的 `EventOutcome` 里，导致下游**无法测**"手势有没有归还捕获"——而漏归还的后果
   （窗口永久霸占鼠标）在行为上表现为"只有一张贴图能拖动"，从现象反推极难定位。

### 实测结果
`cargo test -p windui --lib` 753 项全绿，其中新增 3 项：

| 判据 | 结果 |
|---|---|
| `frameless_outer_size_equals_client_size` | 3 组尺寸×缩放，外框 == `round(w*scale)` ✓ |
| `framed_outer_size_is_strictly_larger` | 带框外框严格更大，高度差 > 8px（**那个差值就是黑边的宽度**）✓ |
| `outer_size_never_collapses_to_zero` | 0 逻辑像素 → (1,1)，避免 `CreateWindowExW` 把 0 当 `CW_USEDEFAULT` ✓ |

排除法提示：`framed_outer_size_is_strictly_larger` 是上面那条的**反向对照**——它证明的是
"绕开 `AdjustWindowRect` 有效"，而不是"这个函数什么都不做"。

### 验收判据
1. 贴图（图片 / 文字）四边贴合、无暗条，四角是直角（真机手工：UIPI 挡住自动化点击，
   且贴图由热键触发，无法脚本化）；
2. 控制面板仍是系统圆角（它不走 frameless 分支）；
3. 全屏覆盖层/选区窗不再在屏幕四角切出缺口。

### 门禁
`cargo test -p windui --lib` 753 ✓；`cargo test`（curseen-ui）**413 + 4 + 2** ✓；
`clippy --all-targets` 0 error / 6 条既有 warning（与补丁前同数）✓；`fmt --check` ✓。

### 「未做」的后续（2026-09-17 已由用户口径关掉，别再往回加）
原计划给**子窗口**加 `.layered()`（逐像素透明）来解锁两件事，用户当天明确否掉了：

① **贴图「右键菜单 → 另存为」**：取消，改由「选中某张贴图 → `Ctrl+S` 弹原生另存为对话框」
   承担（实现见 `main/src/runtime/overlay.rs` 的 `save_pin_as`）。不需要分层，也就没有
   "菜单要盖在所有贴图之上"这个前置难题。
② **贴图窗玻璃边（向外朦胧发光）**：用户明确"玻璃边不要了"。整套 `.layered()` 子窗方案
   随之作废，`PARKED` 的进程级静态也就无需改成按实例。

所以那块 windui 能力**不再有需求**，不必实现。


---

## 补丁 025（app 侧，无需改 windui）：截图/贴图的用户口径改造

用户当天又给了 8 条口径（`1.`–`8.`），全部落在 `main/` 里，windui 一行没动。

### 1) Esc 只关「我选中的那一张」（原来一键全关）

新增**跨窗口唯一的选中态** `SnapMgr::selected_pin`：

| 事件 | 选中态 |
|---|---|
| 左键按下某张贴图（`pin.rs::on_event`） | 选中它（`Runtime::select_pin`），并让上一张重绘掉高亮 |
| 全局钩子收到任意鼠标按下（`Runtime::note_pointer_down`） | 点在该窗矩形之外 → 取消选中 |
| 该窗被关闭（自身双击/右键、Esc、超上限淘汰） | 自动清空 |

Esc 的判据从 `LayerSession::pins > 0` 收紧为 `pin_selected`：**一张都没选中时
`escape_target` 落到 `None`**，用户的 Esc 不再被我们顺手清空贴图——用户报的正是这个。

`note_pointer_down` 刻意做成**几何判定**（`GetWindowRect` 查一次矩形）而不是"等窗口消息"：
贴图窗自己的 `WM_LBUTTONDOWN` 与全局钩子投递过来的按下事件谁先谁后不确定，只有"点落在哪"
与顺序无关。为什么"点别处必须取消"：`Ctrl+S` 是**全局**键，选中态粘住不放就会一直拦下用户
在别的程序里按的 Ctrl+S。

顺带修掉一个陈旧记账 bug：`pin::open` 现在挂 `on_close_request` 摘记账。此前只有控件自己的
`close()` 摘，而 Esc / 超上限淘汰走的是外部 `WM_CLOSE` → 记账里留下陈旧序号（贴图计数虚高、
没到上限就开始淘汰）。

### 2) 贴图落点 = 截图时的原位；剪贴板内容居中

`PinRequest` 增 `origin: Option<Rect>`（冻结帧坐标）。落点算成一个**纯函数**
`pin::pin_placement(bmp_w, bmp_h, screen, origin)`，便于单测：

| 来源 | 落点 | 尺寸 |
|---|---|---|
| 截图并贴图（`Some`） | `screen.x + region.x`（含副屏负坐标） | `region.w × region.h`，**1:1 不缩放** |
| 剪贴板图 / 剪贴板文字（`None`） | 虚拟桌面**居中** | 超屏才按 0.9 等比缩小 |

原位一档刻意**不套 90% 上限**——整屏截图就该整屏贴回去（"位置要一样的"），套上限会变成
1728×972，位置也就"差不多"而不是"一样"了。

### 3–6) 截图态外观

- **进来就整屏压暗**：旧实现只在**已有选区**时压暗，刚进截图态的画面与平时一模一样。
  现在 `sel == None` → 整屏铺一层 `(10,14,18,110)`；框选后改为"选区原亮度 + 选区外压暗"。
- **尺寸只留在选区右下角**：`W × H` 画在选区内侧贴边（贴屏幕右下角时画到外面会出屏）。
- **删除全部文字提示**：底部那条"拖拽框选，松手完成 · Enter 确认 / Esc 取消"提示条整体删除
  （键盘手势照常生效，只是不再有任何文字）。
- **删除放大镜**：光标旁那个 148×100、带红橙描边的小矩形（放大镜 + 准星 + 坐标标签）是用户
  眼里的"遗留物"。同时把选区边界从红橙 `(224,92,58)` 换成主色蓝 `(76,139,245)`、2px 收成 1px
  ——红色在本产品里只出现在这一处。`RgbaBitmap::magnify` 因此没有调用方，保留为纯图像工具。

### 7) 玻璃边取消

用户明确"玻璃边不要了"，`.layered()` 子窗方案作废（见补丁 024 的「未做」后续）。

### 8) 右键另存为 → 选中后 `Ctrl+S`

`file_dialog::save_png(default_name)` 走 `GetSaveFileNameW`（与 `open_image` 同族的经典 API，
不用 COM：只多两个标志位）；PNG 编码直接用已有的 `screenshot::save_png`（`png` crate 已在依赖里）。
派发点在 `overlay.rs` 的通道回调里（不在 `Runtime::frame`——保存对话框必须在 UI 线程且经
`ctx.defer_blocking` 延后，通道回调本身就是 UI 线程的事件分发点；放进 frame 还得等下一帧）。
热键**只在有选中贴图时才看**，按键照常透传（不吞用户的 Ctrl+S）。

### 验收（可复现）

| 判据 | 手段 | 结果 |
|---|---|---|
| Esc 只关选中的那张；没选中时不吃 Esc | `snap.rs::esc_closes_only_the_selected_pin` + `layer/mod.rs::escape_ignores_pins_when_none_is_selected` | ✓ |
| 选中项消失（关窗/淘汰）时选中态自动清 | `selection_clears_when_the_selected_pin_disappears` | ✓ |
| 点别处取消选中；每个窗都能取回自己的位图 | `clicking_elsewhere_clears_the_selection` / `pin_content_is_reachable_by_seq` | ✓ |
| 原位 1:1、整屏不缩、负坐标正确、剪贴板居中+封顶 | `pin.rs` 四条位置单测（数字对数字） | ✓ |
| 点一下即选中 | `pin.rs::left_down_selects_this_pin` | ✓ |
| **进来就整屏压暗** | `region.rs` 像素探针：四角+中心亮度落在 120..190（纯白=255 即失败） | ✓ |
| 选区内原亮度、选区外压暗 | 同上：内部 =255，外部落在 120..190 | ✓ |
| **界面上没有红橙像素** | 全屏扫描 `r≥180 && g≤140 && b≤120` 计数必须为 0（旧边界与放大镜都是这个色） | ✓ |
| **选区之外没有任何深色提示条** | 四条暗化带的最暗亮度必须 ≥120（旧提示条约 60） | ✓ |
| 尺寸标签在右下角 | 右下 90×34 最暗 ≤90；左上（避开 1px 边界）必须 = 255 | ✓ |

`cargo test` **429** ✓；`clippy --all-targets` 0 error / 6 条既有 warning ✓；`fmt --check` ✓。

---

## 补丁 026（app 侧）：原位要「入环」、描边要「分层」、Esc 要「一次只关一张」

用户当天第 2 轮 3 条口径。第 1 条经日志核对**是上一版构建的旧现象**（详见下），
真正要改代码的是第 2、3 条。

### 1) Esc 一次只关一张 —— 已被补丁 025 修掉，本轮只补**取证手段**

用户口径：*"pin 图后，当我使用 esc 时，两张图会一起消失。比起原来有所改善。原来是全部 pin 图
消失，现在是两张。但依然错误。"*

拿用户自己的运行日志（`build/log/curseen-20260917.log`）对账：

| 时刻 | 日志 | 含义 |
|---|---|---|
| 01:56:09 | `Esc 关闭 4 个贴图窗` | **旧构建**（"关闭 N 个"这套文案），当时 4 张存活 → 全关 |
| 01:56:15 | `Esc 关闭 2 个贴图窗` | 同一个旧构建，当时只剩 2 张存活 → 又是全关 |
| 02:29:50.673 | `Esc 关闭选中的贴图窗 #5` | **新构建**（补丁 025 之后启动的进程），一次一张 |
| 02:29:52.279 | `Esc 关闭选中的贴图窗 #6` | 两秒后**另一次** Esc（中间 51.456 有一次点击选中 #6） |
| 02:29:57.744 / 58.776 | `#8` / `#9` | 同上，各是一次独立按键 |

—— 用户说的"原来是全部、现在是两张"正是 01:56 那两次旧构建日志（4 张全关 / 2 张全关，
数量不同只是因为存活数不同），**并不是两种行为**。新构建的每次 Esc 都只关一个序号。

为了让这件事以后**一眼可判**，`pin.rs::open` 的 `on_close_request` 里加了一条
`贴图窗 #{seq} 关闭（记账摘除）`：日志里出现几条、什么序号，就是实际关了几个窗口，
不再需要靠时间戳反推。

### 2) 纯截图也要记原位 —— `RingItem` 把原位存进环

用户口径：*"截图并 pin 图，图片是在原来的位置。但是我们单纯截图，而后 pin 图。却出现在屏幕
中间。说明我们单纯截图的图片，位置信息没有被记录。这是错误的。只要是通过我们的功能截图的，
就要记录信息。"*

**根因（代码级）**：`confirm_snap` 里只有 `ShotPin` 一支调 `pin_push(.., Some(rect))`：

```rust
if mode == snap::SnapMode::ShotPin {          // ← 只有这一支带原位
    self.snap.pin_push(snap::PinContent::Image(cropped), Some(rect));
}
```

`Shot` 一支什么都不做。那张图只能等 250ms 后的 `poll_clipboard` 把它当**外部剪贴板内容**
收进环（日志里的 `剪贴板内容入贴图环`），而那条路给不出原位 → 贴图落屏幕正中。

**改法**（两处）：

1. **环条目自带原位**：`ring: VecDeque<PinContent>` → `VecDeque<RingItem>`，
   `RingItem { content, origin }`。原位从此跟着条目走——用户可能隔几十秒才按贴图键，
   "当次请求"里存不住。`pin_next` 取 `item.origin` 而不是恒 `None`。
2. **纯截图主动入环**：新模式 `SnapMgr::note_shot(content, origin)` —— 入环 + 带原位 +
   游标置 `None`（下次按贴图键就是它），**但不开窗**（用户还没按键）。
3. **认领自己的剪贴板写入**：`confirm_snap` 写剪贴板后记下
   `own_clip_seq = GetClipboardSequenceNumber()`；`poll_clipboard` / `pin_next` 看到这个
   序列号就知道"剪贴板里是我们刚截的那张，已经在环里且带着原位"，不再当外部内容读一遍
   （否则又会用 `origin: None` 覆盖掉）。这一条同时消掉了旧实现里"截图并贴图后，
   同一张图被轮询再收一次进环"的重复条目。

### 3) 描边改成「两层」而不是发光

用户口径：*"我忽然感觉，pin 图时，图片边缘还是要向外发光比较好……我看你的描述，发光功能又
好像比较复杂。那么有没有更好的办法？……又或者，我们增加 2 层边框线？外层灰色，外外层黑色。
这样可能操作既简单又有层次？"*

采纳"两层"这个方向。发光要贴图子窗走**分层**（逐像素透明），而 `layered` 后端的 `PARKED`
是**进程级**原子（覆盖层休眠省 8~33MB 用），给子窗开分层会被覆盖层休眠一起抹掉；两层描边
**不需要任何分层**，只是多画一圈像素。

`FRAME = 2`（物理像素，每边），窗口 = 图片 + 2×`FRAME`：

| 环 | 位置 | 颜色 | 作用 |
|---|---|---|---|
| 外环 | 最外 1px | 近黑 `(24,28,32)` | 压**浅色**背景 |
| 内环 | 第 2 个像素 | 近白 `(246,248,251)` | 压**深色**背景，把图与桌面隔开一档 |
| 内环（悬停） | 同上 | 淡主色 `(150,180,225)` | 悬停反馈 |
| 外环（选中） | 最外 1px | 主色 `(76,139,245)` | "我点的就是这张"（Esc / Ctrl+S 对象） |

关键取舍：**窗口变大、图片的位置与尺寸不变**。`pin_placement` 仍返回图片矩形（截图原位 1:1
的全部既有断言原样有效），`frame_inflate` 只在外面撑出描边。贴屏幕边缘时窗口会往外溢出
2px（外环看不见），但"图的位置一样"是硬口径，不为可见性牺牲它。

内环画在**图片外面**是必须的：画在图片里侧的话，"灰图 + 灰边"照样分不出来。

### 验收（可复现）

| 判据 | 手段 | 结果 |
|---|---|---|
| 一次 Esc 只关一张（N 张存活） | `snap.rs::esc_closes_only_the_selected_pin` | ✓ |
| Esc 关的是**窗口确实存在**的那张 | `take_selected` 内含 `has_pin` 复核 | ✓ |
| 纯截图后按贴图键落回原位 | `snap.rs::pure_screenshot_remembers_its_origin_for_a_later_pin`（数字对数字） | ✓ |
| 纯截图不该自己弹窗 | 同上：`take_pin_request()` 必须为 `None` | ✓ |
| 同一张截图不重复入环、原位取最新 | `recording_the_same_screenshot_twice_only_refreshes_the_origin` | ✓ |
| 环回溯带出条目原位；外部内容无原位 | `pin_request_carries_the_origin_only_for_screenshots` | ✓ |
| 窗口四周各留 `FRAME`，图片仍原位 1:1 | `pin.rs::pin_window_reserves_a_ring_around_the_image` | ✓ |
| **两层描边真的画出来了**（40×30 离屏，纯中灰图） | `pin_draws_two_layers_of_border_around_the_image`：`(0,0)=外环`、`(1,1)=内环`、`(2,2)=图片`，四条边各测一点 | ✓ |
| 选中时外环换主色、内环不变、图片不被盖 | `selected_pin_swaps_the_outer_ring_to_the_accent_colour` | ✓ |

`cargo test` **434** ✓（429 + 5：环原位 2 条、窗口留边 1 条、两层描边像素 2 条）；
`clippy --all-targets` 0 error ✓；`fmt --check` ✓。

---

## 补丁 027（windui 侧）：explorer 重启后重建托盘图标

**背景**：用户提需求时点明要"系统托盘图标在资源管理器重启时可以重建"。托盘区域由
`explorer.exe` 持有，它崩溃 / 被用户重启 / 注销重登后会把**它不知道的**所有图标丢掉。
`windui` 全源码里 `TaskbarCreated` 零引用，所以当时确实没有任何重建路径。

**后果为什么致命**：CurSeen 的覆盖层是无边框、透明、穿透、且**不进任务栏**的 `tool_window`，
关掉控制面板之后进程就没有任何可见入口——托盘是唯一凭据。图标一旦消失：进程在跑、钩子在
收事件、拖尾照常画，但用户点不出来东西，也退不出（只能任务管理器杀进程）。此类失败没有任何
日志或崩溃信息，属于最难自查的那一类。

**Windows 的约定**：`"TaskbarCreated"` 是一个用 `RegisterWindowMessageW` 注册的广播消息，
新 explorer 就绪后广播它，所有托盘程序据此重新 `NIM_ADD`。

**改法**（两处，均在 `platform/win32/`）：

1. `tray.rs` 新增自由函数 `reinstall(hwnd, uid, hicon, tip)`，用 `install` 已持有的
   `hwnd / uid / hicon / tooltip` 重新发一次 `NIM_ADD`。
   - **图标沿用同一个 `HICON`**：它是本进程的内核对象，与 explorer 是否重启无关，不必重建位图。
   - **自由函数而非 `&TrayState` 方法**：与同文件既有的 `notify` / `set_tooltip` 同一理由
     ——`Shell_NotifyIconW` 会经 `SendMessageTimeout` 与 shell 的托盘窗口跨线程通信、期间
     本线程泵入站消息。签名只收拷出来的值（`HWND`/`HICON` 是裸指针 `Copy`，提示文字拷成
     `String`），借用便无处可藏（铁律 6）。配套给 `TrayState` 加了 `reinstall_parts()`
     负责产出这份拷贝。
2. `mod.rs` 的主 `wnd_proc` 加一条 **match guard** 臂。
   - 消息 id 是运行期分配的（`0xC000..0xFFFF`），编译期拿不到常量，故不能用常量臂——
     这正是 `RegisterWindowMessageW` 唯一正确的用法。
   - 两段式：第一段 `app_host().and_then(|h| h.tray.as_ref().map(..))` 取出拷贝、借用随语句
     结束；第二段在无借用状态下调 `tray::reinstall`。
   - `taskbar_created_msg()` 返回 `Option<u32>`，注册失败返回 `None` 时**不匹配任何消息**——
     瞎认一个 `0` 会让 id 为 0 的消息被误判，宁可不重建也不要误重建。

**验收（可复现）**

| 判据 | 手段 | 结果 |
|---|---|---|
| 新 API 参与编译 | `cargo check --all-targets` | ✓ exit 0 |
| 既有 447 条测试无回归 | `cargo test --release` | ✓ 447 passed |

真机手工判据（本机不便自动化，需人工）：任务管理器结束 `explorer.exe` 再重启，托盘图标
应当在数秒内重新出现且能右键唤出菜单；此前该场景是"图标永久消失"。日志里有
`托盘：检测到 explorer 重启，图标已重建` 一行作为确证。


## 补丁 028（windui 侧）：取色器 HEX 框「点击即全选」

> ⚠ **已回退（2026-09-18）**：为验证 wind-ui-rs **原版**取色器能否直接粘贴颜色代码，已把
> `.3rdParty/wind-ui-rust-0.16.1/src/ui/color_picker.rs` 还原为上游 `v0.16.1` 原版
> （md5 `5ff30719f62e52b802a78b53f2eb8ecc`，LF，60098 B），并同步 `main/patches/windui/`。
> 本节描述的"取色器面板 HEX 输入框改用 `.select_all_on_click()`"**当前不再生效**——取色器的
> HEX 框已回到裸 `Element::text_input`。
>
> `TextConfig::select_all_on_click` **基础设施也已一并移除**：`ui/inputs.rs` 的字段声明 +
> `Default` 默认值 + `TextInput` 的 `PointerKind::Down` 臂分支，以及 `ui/mod.rs` 的
> `Element::select_all_on_click()` 构造器。故 windui 源码里**已不存在任何 `CurSeen 补丁 028`
> 标记**，本节仅作历史记录保留。
>
> 另：上游 `color_picker.rs` 从 **0.16.1 到 0.19.0 逐字节未变**（两版同一 md5），故"升级到
> 0.19.0 就能看到不一样的取色器"不成立——0.19.0 的取色器与本节回退后的 0.16.1 完全相同。
> 原文见备份 `main/backups/colorpicker_patches_20260918/`（含 `.patched` 文件与 unified diff）。

**背景**：用户实测"颜色里面是 HEX……我试着手动输入，结果发现没有任何反应"。

**先复现，再动手**。既有用例 `typing_a_hex_value_updates_the_color` 的注释写得很明白：
「HEX 框直接写它绑定的文本信号（TextInput 没有 on_change 回调），故这里也直接写」——
也就是说**整条链路的头一段（指针 → 焦点 → 按键 → 控件）从来没被验过**。补一条真的用键盘
打的用例，立刻红，且诊断把断点钉在第 4 步（控件自己那一步，不是焦点也不是信号）：

```
DIAG input=#8 widget_is_textinput=true old="#000000"
DIAG backspace consumed=true   text="#000000"      ← 退格"被消费"却什么都没删（光标在 0）
DIAG type-Z    consumed=true   text="Z#000000"     ← 字符插到了**串首**
```

**根因**：HEX 框里预填着当前颜色的 `#RRGGBB(AA)`，而单击只把光标放到点击处。用户点它
是想**换一个值**，可无论插在串首（`Z#000000`）还是串尾（`#000000Z`），拼出来的都不是
合法颜色 → `Color::from_hex_str` 解析失败 → 颜色纹丝不动。用户看到的就是"手动输入没有
反应"。这不是"输入框没拿到焦点"，是**字段语义被按普通文本框实现了**。

**改法**（三处，均带 `CurSeen 补丁 028` 标记）：

1. `ui/inputs.rs` `TextConfig` 新增 `select_all_on_click: bool`（默认 **false**）。
2. `ui/inputs.rs` 的 `PointerKind::Down` 臂：在落到"把光标放到点击处 + 起拖选"之前，
   若开了这一位则 `select_all()` 并 return。双击选词 / 三击选段在**上面**已经 return，
   不受影响（点完再按方向键/Home/End 仍可逐字定位）。
3. `ui/mod.rs` 加 `Element::text_input(..).select_all_on_click()`；取色器面板的 HEX
   输入框改用它。

**为什么是"全选"而不是"自动清空"**：清空会让用户失去"看一眼现值再改一位"的能力，而全选
之后键入即整体替换（`type_char` 先 `delete_selection` 再插入）——与各家取色器/代码编辑器
的 HEX 字段一致。默认关闭：普通表单框点击即全选会让人没法用鼠标定位光标。

**验收（可复现）**

| 判据 | 手段 | 结果 |
|---|---|---|
| 键盘键入 HEX 能改色 | `cargo test --release --lib typing_on_the_keyboard_into_the_hex_box` | ✓（改前红，改后绿） |
| 普通输入框未被改坏 | `… a_plain_text_input_still_places_the_caret_on_click` | ✓ |
| 取色器 21 条用例无回归 | `cargo test --release --lib color_picker` | ✓ 21 passed |
| 整仓无回归 | `cd main && cargo test --release --lib` | ✓ 517 passed |

---

## 补丁 029（windui 侧）：取色器 HEX 框两修 — 手动输入不被补全 + 拖拽选色后文本同步

> ⚠ **已回退（2026-09-18）**：同补丁 028。`HexField` 包装层、`ColorTrigger::hex_focused` /
> `hex_was_focused` 字段、焦点感知版 `on_update` 与 `normalize_hex`、以及本节新增的
> `hex_box_keeps_partial_input_while_focused_and_normalizes_on_blur`（含 `NullCanvas` /
> `render_once` 辅助）**均已从 windui 源码删除**。当前取色器 HEX 框的同步逻辑即上游
> `v0.16.1` 原版 `ColorTrigger::on_update`（无焦点感知、无 `hex_focused`）。
> 回退动因：用户要求在"原版能否直接粘贴颜色代码"这一问题上拿到一手结论——若原版可粘贴，
> 就直接采用原版；本节的焦点感知方案会让"拖颜色条时 HEX 文本停止实时更新"。
> 备份见 `main/backups/colorpicker_20260918/`（`color_picker.*.rs` + `mod.patches_src.rs`）。

### 问题（用户报告，2026-09-18）
虚拟键盘模块的颜色选择面板里，HEX 输入框（a.颜色输入框）有两个 Bug：
1. **手动输入时被反复补全长度**：在框里手动敲 `#FFF`，每隔很短间隔文本就被自动补成
   `#FFFFFF`，无法自由输入。
2. **拖拽选色后文本不更新**：操作 b（SV 方块）/ c（色相条）/ d（透明度条）选色时，下拉
   菜单里的颜色文本会变，但 HEX 输入框（a）的文本纹丝不动。

### 根因
- 原 `ColorTrigger::on_update` 用 `hex.get() != echo.get()` 判定「用户在打字」。但
  `commit_color` 会把 echo 写成**完整**色值（如 `#FFFFFF`），于是每帧 `text != echo`
  持续为真，触发 `push_hex` 把 `#FFF` 补成 `#FFFFFF`——正是 Bug 1。
- 原 `on_update` 末尾回写 HEX 文本的守卫 `hex.get() != s && hex.get() == echo.get()`
  永远不成立（echo 早被用户原文覆盖），于是 Bug 2：拖拽选色后 HEX 框文本不更新。
- 框架不向控件投递焦点事件（`Event` 只有 `Pointer`/`Key`），无法直接感知「失焦」。

### 改动文件
- `src/ui/color_picker.rs`：
  - 新增 `HexField` 包装 `TextInput`（CurSeen 补丁 029），在 `paint` 里用
    `was_focused: Cell<bool>` 比对 `focused` 参数，识别「刚获焦 / 刚失焦」两个瞬间，
    把焦点态写进共享信号 `focused_sig`；`reset_interaction` 里把焦点信号归位（面板收起时
    节点销毁，避免误判仍在编辑）。
  - `ColorTrigger` 新增 `hex_focused: Option<Signal<bool>>` 与 `hex_was_focused: bool`。
  - 重写 `on_update`：**焦点在**时用户独占文本，绝不回写（避免打断输入）；**失焦那一瞬**
    调用 `normalize_hex` 把文本规范成完整色值（含 `#FFF`→`#FFFFFF`）。文本未变而
    `color_changed` 为真时，把标准色值同步进 HEX 框（修复 Bug 2）。
  - `push_hex` 更名为 `normalize_hex`，只承担「失焦时规范化」。
- `src/ui/mod.rs`（`color_picker_opts`）：创建共享 `hex_focused` 信号；HEX 输入框改用
  `HexField::new(...)` 构建；`ColorTrigger::new` 多传一个 `hex_focused` 参数。

### 验收（可复现）
| 判据 | 手段 | 结果 |
|---|---|---|
| 手动输入不被打断 | 焦点在时键入 `#FFF`，文本保持 `#FFF` 直到失焦 | ✓ 不再自动补全 |
| 失焦才补全 | 失焦后 `#FFF` → `#FFFFFF` | ✓ |
| 拖拽选色后 HEX 文本更新 | 拖 SV 方块 / 色相条 / 透明度条，断言 `hex_text` 跟随 `value` | ✓ |
| 取色器全部用例无回归（含本补丁新增的焦点回归） | `cargo test --release --lib color_picker` | ✓ |

## 补丁 030（windui 侧）：取色器 HEX 框重新落地「能打字 + 拖动实时」

**背景**：028/029 回退成原版后，用户实测得到的是：**拖动时不实时、松手才更新；整串粘贴可用、
手动打字不行**。诉求是"两者都要"——手动输入不被实时校正，同时拖拽时文本跟着走。

### 一手的探针结论（下面几条门禁就是由它来的）

| 事实 | 探针判据（Down→Move 不送 Up / 直接写信号） |
|---|---|
| 拖动中**状态层本来就更新** | `hex_text` 从 `#1A1717` 变到 `#E51717`，与 `value.to_hex_string()` 一致 |
| 半截输入**第 2 帧就被回写覆盖** | `#2F` → 第 1 帧仍是 `#2F`、第 2 帧变 `#000000` |
| 整串粘贴**本来就能用** | `#2F9E44` → 提交成颜色且文本保持 |

⇒ 结论：**"拖动不实时"不是状态算错，是那一帧根本没重绘/重排**（见下）。

### 根因

- **（b）拖动不实时**：`Move` 是 hover 高频路径，框架**刻意不为它置 `needs_relayout`**
  （`core.rs::call_on_event`：事件期写过信号的 `Move` 只给 `Rect` 级失效）。于是拖动帧
  `layout_root` 不跑 → 响应式 `ColorTrigger::on_update` 也不跑 → HEX 文本停住；松手走的是
  click 路径（会置 `needs_relayout`），于是"一松手才跳到位"。
  ★ windui 自己就有同款先例与既有用例：`core.rs::stepper_follows_value_on_repaint_only_frames`
  ——注释写的就是"拖滑块全程是 `Move`…数字会一路纹丝不动，直到松手才一次性跳到位"。
- **（a）打字打不完整**：原版打字分支只要 `from_hex_str` 返回 `Some` 就提交并放行，可
  `from_hex_str` **认 3 位简写**，而 `#2F9` 既是合法简写、又正是 `#2F9E44` 敲到第 4 个字符
  时的样子。一旦提交，闸门放开，下一帧 `push_hex` 就把框改写成标准色值（`#22FF99`），
  用户后面三个字符全丢。就算把简写排除，原版"解析不出来就不提交、但也不冻结"同样致命：
  那个 `return` 之后，同一帧的 `push_hex` 依旧会拿当前色去比文本并覆盖。

### 改动

- `src/ui/color_picker.rs`：
  - `ColorTrigger` 新增两个字段：`hex_pending: bool`（框里是用户的半成品 → 冻结回写）与
    `hex_pushed: Option<Color>`（上一次写进框的颜色，用来分辨"颜色变了"与"用户还在打字"。
    **不能借 `PickerState::echo`**：`commit*` 会同时写 `value` 与 `echo`，`value == echo`
    恒为真，两种情况分不开）。
  - 新增 `ColorTrigger::is_finished_hex`：**只有 6 / 8 位十六进制才算"打完"**，3 / 4 位简写
    一律按"还在打字"处理。
  - `on_update`：打字分支改为「是打完的色码 → 提交并解冻；否则 `hex_pending = true`」；
    末尾 `push_hex` 之前加闸门「`hex_pending` 且颜色未变 → 直接返回，别碰用户的草稿」。
  - `push_hex` 改为 `&mut self` 并记录 `hex_pushed`。
  - 三条拖拽通道（`SvArea` / `HueBar` / `AlphaBar`）的 `Move` 分支各加一句
    `ctx.mark_layout_dirty()`：把失效从 `Rect` 升为 `Layout` → `apply_damage` 据此置
    `needs_full` → 全窗帧必先 `layout_root` → 同步在本帧内完成。

### 验收（可复现）

| 判据 | 用例 | 结果 |
|---|---|---|
| 三条拖拽通道的 `Move` 帧失效足以带动重排 | `every_drag_channel_escalates_damage_so_followers_repaint` | ✓ |
| 按住不放（不送 `Up`）文本已更新 | `dragging_without_release_already_updated_the_hex_box` | ✓ |
| 逐字符敲 `#2F9E44` 全程不被打断、不跳色 | `typing_a_partial_hex_is_not_rewritten` | ✓ |
| 半截串在手时颜色被别处改动 → 框让位给新色 | `a_color_change_after_partial_typing_takes_over_the_box` | ✓ |
| 整串粘贴提交且文本保持 | `pasting_a_full_hex_commits_and_is_kept` | ✓ |
| 取色器全量无回归 | `cargo test --lib color_picker` | ✓ 24 passed / 0 failed |

### 说明与代价

- **文本里敲 3 位简写不再即时生效**：`#2F9` 不会改色。这是"不误判用户还没打完"的必要代价；
  `Color::from_hex_str` 本身仍认简写，配置文件等其它调用方不受影响。
- **拖拽期间每帧走整窗帧**（`Layout` 在 Layer 1 暂等价整窗）。取色器只在设置面板里，面板
  窗口很小，代价可接受；换来的是"拖动全程文本实时"。
- **本补丁可原样移植到 0.19.0**：`color_picker.rs` 在两版里逐字节相同（md5 `5ff30719…`），
  `mod.rs` 的 HEX 框接线也一致。

## 基线版本决策（2026-09-18）：**不迁移**到 wind-ui-rust 0.19.0

动因：用户怀疑"0.16 的取色器被补丁改坏了"，下载了 `.3rdParty/wind-ui-rust-0.19.0`
并倾向把 UI 基线换成 0.19。结论是**不换**，依据如下（全部可复现）。

| 事实 | 证据 |
|---|---|
| **0.19.0 里没有 `layered` feature**，main 的依赖声明无法解析 | 在 0.19.0 目录跑 `cargo check --no-default-features --features layered` → `error: the package 'windui' does not contain this feature: layered` |
| **0.19.0 的 win32 平台没有任何逐像素透明窗口机制** | 全树计数：`WS_EX_LAYERED`=0、`UpdateLayeredWindow`=0、`SetLayeredWindowAttributes`=0、`BLENDFUNCTION`=0、`WS_EX_NOREDIRECTIONBITMAP`=0；窗口扩展样式里只有 `WS_EX_TOOLWINDOW`×3 |
| 我们的覆盖层**不是**"上游能力 + 补丁"，是**自有 fork 能力** | `WS_EX_LAYERED` / `UpdateLayeredWindow` / `layered` / `DibTarget` / `BlendMode` / `tool_window` / `PixmapMut` 在 **pristine v0.16.1 里全是 0**（= 全由我们引入），在 **0.19.0 里也全是 0**（上游从未采纳） |
| 迁移要重做的东西有多大 | main 依赖 **11 项** 0.19 缺失的 API：`BlendMode`、`with_blend`、`tool_window`、`set_click_through`、`clear_rect`、`no_activate`、`slider_value_range`、`DibTarget`、`fill_polygon`、`fill_ellipse`；外加自有文件 `platform/win32/layered.rs`（431 行）与散落在 18 个文件里的约 1700 行新增 |
| **取色器与版本无关** | `color_picker.rs` 在 0.16.1 与 0.19.0 里**逐字节相同**（md5 `5ff30719f62e52b802a78b53f2eb8ecc`）——所以"换 0.19 才能看到原版取色器"这个动机本身不成立：现在跑的就是原版 |

**决定**：继续以 `0.16.1 + 补丁 002~027 + 030` 为基线。迁移 0.19.0 的唯一已知收益
（"看到原版取色器"）已于 2026-09-18 达成，其余全是净代价。

### 复现脚本

- `temp/upstream/_migration_matrix.py`：`main` 用量 × 我们这份 × 0.19 的存在性矩阵
- `temp/upstream/_audit_patches_vs_019.py`：逐文件抽出"相对 pristine 新增的独有符号"再查 0.19
- 原始输出：`temp/upstream/migration_matrix.txt`、`temp/upstream/audit_019.txt`
- pristine 基线：`temp/upstream/wind-ui-rust-0.16.1/`（v0.16.1 tarball 解出）

### 顺带发现（待办）

**有 6 个被我们改过、却没有 `CurSeen 补丁 0NN` 标记的文件**：`core.rs`、`lib.rs`、
`sync.rs`、`text/coretext.rs`、`text/dwrite.rs`、`ui/containers.rs`。改动本身大多是补丁
015（`PixmapMut`）/008（`WindowOp::SetClickThrough`）/011（通道消费权）的**配套面**，
但缺标记意味着 `tests/docs_consistency.rs` 的"标记 ↔ 登记章节"对账**看不见它们**，
将来升级依赖时最容易丢。补标记要连同登记章节一起补，否则门禁会红——留作后续。

## 基线版本决策（2026-09-20）：**迁移到 0.19.0**，删除 0.16.1

上一节（09-18）的结论是"不迁移"，本次按用户要求**推翻**：统一第三方库到
`.3rdParty`，并把 UI 基线切到 **`wind-ui-rust-0.19.0`**，`wind-ui-rust-0.16.1` 已删除
（fork 备份在 `temp/upstream/wind-ui-rust-0.16.1.fork.backup/`，0.19  pristine 备份在
`temp/upstream/wind-ui-rust-0.19.0.pristine/`）。

### 关键前提：0.19 仍然是我们的 fork，不是"换成上游"

上一节列的事实**依然成立**——0.19.0 没有 `layered` feature、win32 侧没有
`WS_EX_LAYERED` / `UpdateLayeredWindow` / `DibTarget`。所以这次不是"换个版本号"，
而是**把我们的 fork 重新建立在 0.19 之上**：

- 给 0.19.0 的 `Cargo.toml` 加回 `layered = []`（src 补丁覆盖不到，必须手加）；
- 迁移相对 pristine 0.16.1 的改动：**18 个文件 + 新增 `layered.rs`（431 行）**，
  合计约 **1835 新增行 / 127 删除行**。

### 迁移方法（可复用）

```bash
diff -ruN pristine0.16.1/src fork0.16.1/src > our_fork.patch   # 算出我们的改动
cd .3rdParty/wind-ui-rust-0.19.0 && patch -p1 --fuzz=5 < our_fork.patch
```

结果：**74 个 hunk，66 个自动落上（89%），8 个失败** → 看 `.rej` 手工补。

### 8 个失败 hunk 的处置

| 位置 | 内容 | 处置 |
|---|---|---|
| `app/mod.rs` #10/#11 | 补丁 011（建主窗即认领通道消费权） | 手工落到主窗 `UiHost::new` 之后（`let mut host` + `host.title_src` 后） |
| `app/mod.rs` #15 | 补丁 026（子窗 `shortcut`） | **删除**——上游 0.18 已原生实现，见下 |
| `win32/mod.rs` #3 | import（`GetCursorPos` / `RegisterWindowMessageW` / `WS_VISIBLE` / layered 四件套） | 手工补 import |
| `win32/mod.rs` #6 | 补丁 010（唤醒过滤） | 自动落上（fuzz） |
| `win32/mod.rs` #20/#21 | 补丁 017（托盘菜单搬出 UI 线程） | 手工：RightClick 改调 `show_tray_menu(menu)`，`WM_TRAY_RESULT` 分支改收 `Option<HWND>` |
| `tray.rs` #2 | `WM_TRAY_RESULT` 常量 + import | 手工补 |

### ★ patch 必要性检查的结论：补丁 026 已被上游吸收

**补丁 026（`Window::on_shortcut` / `WindowRequest::shortcut`）在 0.18.0 已被上游原生
实现**（CHANGELOG 0.18.0「常驻模式补齐四样」之 `Window::on_shortcut`）。0.19.0 的
`build_new_window` 里那句注释写的就是我们的语义（"快捷键取这个窗口自己声明的那一份"）。

故移植时**必须删掉我们那份**，否则 `WindowShortcutHandler` / `shortcut` / `on_shortcut`
三处重复定义。这正是"检查 patch 在新版本是否还有必要"的价值——不是所有 patch 都要搬，
能力上游化了的要删。

其余 patch（003/004/006/007/008/010/011/012/015/016/017/018/022/024/027/030）在 0.19
上仍然是必需的：0.19 依旧没有分层透明、没有唤醒配速、没有 explorer 重启重建托盘。

### 0.19 的 API 变化带来的适配（非 patch 冲突）

- `AppHost.main` 从 `HWND` 变 **`Option<HWND>`**（常驻模式）→ `run_tray_actions(main, …)`
  直接收 `Option`，托盘结果分支不再自己判空句柄。
- `UiHost::new` 主窗路径多出 `host.title_src = …`，且是 `let mut host`。
- `track_menu` 在 pristine 0.19 是 2 参数（内部 `GetCursorPos`），我们补丁 016 改成
  3 参数（外部传 `POINT`）→ 调用点随之改为走 `show_tray_menu`。

### 验收

- `cargo check --release`：通过（仅 nnnoiseless 的 `cargo_c` 无害 warning）。
- `cargo test --release --lib`：**575 passed / 0 failed / 1 ignored**（与 0.16 基线一致）。
- 分层能力接线核验：`layered.rs` 18,923 字节（与 fork 同）、`pub(crate) mod layered;`
  已注册、`UpdateLayeredWindow`=18 / `DibTarget`=7 / `WS_EX_LAYERED`=6（与 fork 同）、
  `CurSeen 补丁` 标记 75 处。

## 补丁 031（windui 侧 + app 侧）：覆盖层全屏压暗/实色填充从 11ms → 1.8ms

**背景**：用户报「鼠标拖尾和悬浮标一开就卡、CPU 从 0 跳到 8%」。帧成本探针
（`tools/probes/frame_cost_probe.rs`）逐段拆解 1920×1080 的一帧，定位到**单项最大头**：
聚光灯暗罩（`assets/HoverDot/spotlight.lua` 旧实现）用一次「盖满全屏、中心挖洞」的环带
多边形填 `ctx:polygon`，光栅器要对几乎整屏每个像素算覆盖率 —— 实测 **11.5 ms/帧**，占
60fps 单核的 71%。拖尾（pixie_dust）本身只 ~0.85 ms，不是瓶颈；是聚光灯把整帧拖到 13ms。

### 两处改动

**① windui `render/skia.rs`：给轴对齐整数矩形的实色填充加快路（补丁 009 的姊妹条）**

补丁 009 只覆盖了 `BlendMode::Clear`（整块清除）。实色 `SourceOver` 仍一路走到
`rounded_rect_path → fill_path`（通用扫描线光栅器）：一个轴对齐整数矩形也要 11ms。

新增 `fill_device_rect_solid`：整数对齐 + 无裁剪 + 无离屏层时按行直写，跳过光栅器。
两分支：
- 源不透明（`sa==255`）→ 整行写同一 4 字节模式（等价于 tiny-skia 的 memset 快路）；
- 半透明 → 逐像素 premultiplied SourceOver；**特例**：目标像素完全透明时 SourceOver 退化
  为「直接写源」，省掉乘加与 `div255` —— 覆盖层暗罩层每帧都从透明画起，走这条，否则全屏
  半透明压暗仍要逐像素混合（这是 11ms 砍到 1.8ms 的关键一跳）。

`div255` 照抄 tiny-skia lowp 管线的 `(v+255)>>8`，保证与光栅器逐像素相同。

**② app 侧 `spotlight.lua` 改写 + 新增 `clear_circle` 原语**

旧：整屏多边形。新：两步都避开整屏光栅 ——
- `ctx:rect(0,0,screen_w,screen_h, 0,0,0,dim)` 整屏压暗：轴对齐矩形，走补丁 031 快路；
- `ctx:clear_circle(cx,cy,r)` 在暗罩上挖洞：Clear 混合只光栅化一个小圆（r=140 时
  ~280×280 包围盒）。

为此在 `core/draw.rs` 的 `Blend` 枚举加 `Clear` 变体，`render.rs::to_blend` 映射到
`WBlend::Clear`，`scripting/api_draw.rs` 加 `ctx:clear_circle`（忽略源色，只清成透明）。
`Paint::Solid` 自带的 `BlendMode` 枚举本来就有 `Clear`，无需改。

### 验收（硬证据）

- `tools/probes/frame_cost_probe.rs` 复测（`cargo run --release --example frame_cost_probe`）：
  光栅成本 11.46ms → **1.82ms**；聚光灯单开一帧 13.42ms → **3.56ms**（60fps 占核 80%→21%）；
  拖尾+聚光灯一帧 11.85ms → **4.41ms**（占核 71%→26%）。
- `solid_rect_fast_path_matches_the_rasterizer`（windui 单测，5 组用例含不透明/半透明/
  极低 alpha/整屏/a=254）：快路与光栅器全缓冲最大通道差 **≤1/255**，验证等价性而非"看起来一样"。
- `spotlight_lua_punches_a_real_hole_in_the_dim`（app 单测）：中心 α=0、四角 α=dim、
  透亮像素数 = πr²（±8%），新写法视觉与旧多边形**逐像素一致**。
- `cargo test --release --lib`：643 passed / 0 failed / 4 ignored（无回归）。
- 部署：`cargo build --release` + `tools/deploy.py` 同步 `spotlight.lua` 与 windui 改动。

## 补丁 032（windui 侧 + app 侧）：聚光灯增量帧 —— 局部重画 + 局部呈现，4K 一帧 8.8ms → 0.5ms

日期：2026-09-20。

### 背景

补丁 031 把聚光灯"整屏多边形"光栅降到 ~1.5ms@1080p，但成本仍随屏幕像素数线性放大：
4K 实测整窗路径一帧 ~9ms（清屏 3.6 + 光栅 0.5 + 换序 4.8），60fps 占核 53%，多屏/更高
分辨率还会更糟。而聚光灯的画面**每帧几乎全没变**——整片压暗是恒定色，动的只有光圈那个洞。

### 方案：增量帧（脏区 = 旧洞 ∪ 新洞）

**① windui 侧（`platform/win32/layered.rs`）**

- `request_partial_present(x,y,w,h)` + 私有 `take_partial_present()`：宿主在 widget
  paint 期间登记本帧脏区（设备像素），后端 render 返回后取走消费，一次一帧。
- `buffer_generation()`：后备缓冲代数，`ensure` 重建 / `release` 释放时 +1。宿主据此
  作废自己的增量缓存（休眠、resize 后上一帧内容不复存在）。
- `swizzle_region_in_place`：R/B 换序只做脏区（脏区外上一帧已是 BGRA，重碰会翻回
  RGBA 语义）。独立自由函数便于单测。
- `present_rect`：`UpdateLayeredWindow` 的**子矩形更新**——整张 DIB 仍选进内存 DC，
  `ptSrc`/`pptDst` 给出子矩形偏移、`psize` 给出子矩形大小，合成器只重合成那一块。
- `paint` 分派：fresh 帧（缓冲刚重建）或无脏区 → 整窗换序 + 整窗呈现；有脏区 →
  `swizzle_region` + `present_rect`。park 过渡帧作废挂起的脏区请求。
- 顺手修了 `app/mod.rs` 一个在 `--features layered` 下编译不过的既有测试
  （`WindowState` 缺 `click_through` 字段）。

**② app 侧（`runtime/mod.rs` + `runtime/overlay.rs`）**

- `Runtime::detect_spotlight_partial`：指令表组装完后识别「整屏实色矩形(SourceOver)
  + 圆形 Clear 挖洞」的两指令形态（即 spotlight.lua 的产物，不写死脚本名）。命中且
  上一帧暗罩同色时，把指令表**替换**成三指令增量版：
  `Clear(脏区)` → `Fill(脏区, 暗罩色)` → `ClearCircle(新位置)`。清过底再填，SourceOver
  落在透明上，与整窗路径逐像素一致。
  - 脏区 = 旧洞 ∪ 新洞外接矩形，外扩 2px 容抗锯齿边，裁进覆盖层窗口。
  - 作废条件（自动回整窗）：形态不符（任何其它模块出指令）、暗罩变色、缓冲代数变、
    GIF 选框边框画过（overlay 直接往缓冲上画，缓冲不再等于"暗罩+光圈"）。
- `OverlayCanvas::paint`：增量帧跳过整窗清屏，把世界坐标脏区经 `to_device`
  （与 `render::View` 同一套映射）换算成设备像素后调 `request_overlay_partial_present`；
  其余帧维持原「清屏 + 整窗重画 + 整窗呈现」。

### 验收（硬证据）

- `tools/probes/spotlight_partial_probe.rs`（生产同一套 detect 代码，120 帧均值）：
  | 分辨率 | 整窗一帧 | 增量一帧 | 60fps 占核 |
  |---|---|---|---|
  | 1920×1080 | 2.11 ms | **0.48 ms**（77%） | 12.7% → **2.9%** |
  | 2560×1440 | 4.28 ms | **0.45 ms**（90%） | 25.7% → **2.7%** |
  | 3840×2160 | 8.84 ms | **0.53 ms**（94%） | 53.1% → **3.2%** |
  增量命中 119/120（首帧种缓存）。
- `incremental_frame_matches_full_redraw_pixel_wise`（app 单测）：保留上一帧缓冲、
  只画脏区的增量帧，与"清屏 + 整窗重画"**逐字节一致**（含抗锯齿边）。
- `region_swizzle_matches_full_swizzle_inside_and_preserves_outside` /
  `out_of_bounds_region_is_a_noop`（windui 单测）：区域换序与逐行手工换序逐字节
  一致，越界裁剪正确。
- 脏区/拒判/首帧/变色四个行为单测（`spotlight_partial_tests`）。
- 顺手修了 `virtual_keyboard::release_starts_hold_window` 的环境敏感断言：标签断言
  改按**键码**找（原按 `"a"` 标签找，真机 CapsLock 开着时标签是 "A"，测试随环境翻红）。
- `cargo test --release`：649 passed / 0 failed（含 docs_consistency 补丁登记门禁）。
- 部署：`cargo build --release` + `tools/deploy.py`。

### 补丁 032 修订（2026-09-20 15:46）：真机回归——特效全部消失，两处修正

真机验收发现拖尾/悬浮标/暗罩**全部消失**。根因两处，均在首版 032：

1. **`present_rect` 误用 `UpdateLayeredWindow` 语义（致命）**：`pptDst`/`psize`
   指定的是**窗口自身**的位置与尺寸，不是"更新区"——分层窗**没有子矩形更新**。
   首版把脏区递给 `psize`，每个增量帧把整个覆盖层窗口缩小成那一小块，
   屏上其余内容全没。已删除 `present_rect`，呈现**恒为整窗**（`present`）；
   局部化的收益只保留在"跳过整屏清屏 + 只画脏区 + 只换序脏区"。
2. **`OverlayCanvas::paint` 顶部无条件清屏**：把增量帧赖以生存的"上一帧缓冲
   内容"每帧抹掉。清屏移入非增量分支，与"整窗重画"配对；增量帧不清屏。

教训入档：windui 分层窗的局部化只能省 CPU 侧（光栅/换序），提交侧 ULW 恒整窗；
任何"局部更新"想法都必须先查 Win32 参数语义，不能望文生义。
部署：build/curseen2.exe 15:46，md5 f1e7e4c8…。

## 补丁 083（windui 侧 + app 侧）：GIF 选区的区域级命中 + 悬停光标变化

日期：2026-09-20。

### 背景

整个覆盖层窗口恒置 `WS_EX_TRANSPARENT`（整窗穿透），无法表达「选区框内/框外穿透、
但 8 个手柄点与边框本身可按住拖拽」这种**区域级**交互。补 NCHITTEST / 光标两种
区域钩子，CurSeen 侧据此实现 GIF 录屏选区的 8 手柄 + 边框移动交互。

### 方案

**① windui 侧（083-a..d）**

- `event.rs`（083-a）：新增 `pub enum NcHit { Client, Transparent }`；`CursorShape`
  追加 `SizeNWSE`（对角缩放）/ `SizeNESW`（反对角缩放）/ `SizeAll`（四向移动）。
- `platform/mod.rs`（083-b）：`AppHandler` 追加默认返回 `None` 的 `nchit_at(hwnd, pt)`
  与 `cursor_at(hwnd, pt)` 钩子，宿主可覆盖。
- `app/mod.rs`（083-c）：`App` 新增 `nchit_hook` / `cursor_hook` 字段与 builder
  `on_nchit(f)` / `on_cursor_at(f)`（`impl Fn(windui::geometry::Point) -> ...`）；
  `AppHandler for UiHost` 转发给这两个开关。
- `platform/win32/mod.rs`（083-d）：`WM_NCHITTEST` guard 放宽为无条件调
  `handle_nchittest`，其顶部调 `nchit_at` 钩子（命中非 None 时用 `hit_code(NcHit)`
  把 `Client→HTCLIENT`、`Transparent→HTTRANSPARENT` 返回）；`WM_SETCURSOR` 经
  `cursor_pos_client(hwnd)` + `cursor_at` 钩子取悬停光标；`apply_cursor` 补
  `SizeNWSE→IDC_SIZENWSE` / `SizeNESW→IDC_SIZENESW` / `SizeAll→IDC_SIZEALL`。
- 区域级命中前提：只有 `WS_EX_TRANSPARENT` **清置**后 NCHITTEST 才触发（OS 命中
  阶段会直接跳过置位窗）。故 CurSeen 需先把覆盖层切成三态穿透。

**② app 侧（`core/gif_zone.rs` + `runtime/mod.rs` + `runtime/overlay.rs`）**

- `core/gif_zone.rs`（2a）：纯几何 `hit_gif_zone(rect, x, y, handle_px, border_px)`
  → `HitZone`（None/Body/四角/四边中点，角优先），含负原点、退化矩形判空；6 个单测。
- `runtime/mod.rs`（2b）：`gif_interaction_enabled()`（选区存在且未录制且无活动层）；
  `gif_resize_or_move(zone, dx, dy)` 增量拖拽（Body 平移、四角双轴缩放、四边单边缩放，
  全部 clamp 到虚拟桌面并保证 `GIF_ZONE_MIN_SIZE`）；4 个单测（含负原点屏幕）。
- `runtime/overlay.rs`（2c/2d/2e/2f）：穿透变量改三态
  `MouseMode { Transparent, Interactive, GifRegion }`，`sync_mouse_mode` 优先级
  GifRegion > Interactive > Transparent，仅 Transparent 置 `WS_EX_TRANSPARENT`；
  `draw_gif_frame` 按外框画 8 个手柄方块（四角+四边中点，同色）；`OverlayCanvas`
  `on_event` 实现拖拽状态机（Down/Move/Up）；`run_overlay` 注入 `on_nchit`/`on_cursor_at`
  钩子——手柄/边框回 HTCLIENT 且悬停光标变化，框内/框外回 HTTRANSPARENT 穿透。
  录制中 `gif_interaction_enabled()` 判 false，交互整体禁用。

### 验收（硬证据）

- `cargo test --lib`：667 passed / 0 failed / 4 ignored（含 `gif_zone` 6 个、
  `runtime::gif_` 相关新增）。
- `cargo fmt` / `clippy` 通过；`cargo build --release` + `tools/deploy.py` MD5 一致。


## 补丁 084（**app 侧**，无需改 windui）：截图编辑态 —— `Ctrl` + 截图热键，框完停顿可调可画

日期：2026-09-23。

### 背景

用户口径：*"截图时，如果按住 Ctrl，选区选择后应该停顿，就像 gif 选区一样，允许选区缩放，
也允许选区整体移动……并且，按住 ctrl 键停顿时，我希望可以对截图做简单处理，就像屏幕标注一样。
换言之，**截图和屏幕标注在本质上是相同的函数**。……画笔工具栏在截图功能里就不能总是处在屏幕
中央下方了。可能还得跟随截图区域，显示在附近。"*

即：把「截图」从"框完即出图"扩成"框完停顿 → 可微调选区 → 可标注 → `Enter` 出图"。
底层不新建管线：`core/backdrop.rs` 的模块头本来就写着这句原话（"一份冻结帧 + 怎么把它铺到
画布上"），**加一个消费方**而已。

### 方案（纯 app 侧；windui 一个字节没动）

- **新 `runtime/shot_edit.rs`**：`ShotEdit{sel, frame, pin, drag}` 会话 +
  `frame_rect(screen)`（帧内偏移 = 世界坐标 − 虚拟桌面原点，四边钳进帧并取整） +
  `compose(e, ink, screen)`（笔迹离屏 `DibTarget` 光栅 → 预乘 source-over 到"冻结帧裁到选区"
  的底图；`scale = 1.0` 与覆盖层实时渲染**同源**，故产物与屏上逐像素一致）。7 个离屏单测。
- **`core/gif_zone.rs`**：新增 `HANDLE_LOGICAL` / `FRAME_LOGICAL` / `BORDER_LOGICAL` 与
  `hit_zone_at(rect, x, y, dpi)` —— 绘制端（`draw_gif_frame`，逻辑 dp）与**两个**命中端
  （覆盖层 NCHITTEST / 光标 + `Runtime::edit_drag` 的全局钩子事件）共用同一套换算。
  `overlay.rs` 的本地常量 `GIF_FRAME_PX` / `HANDLE_PX` 删除并改引这里。
- **`runtime/mod.rs`**：`SnapMode::ShotEdit{pin}` 分岔（判 `pending_mode()`，**不裁帧**，
  整张冻结帧交给会话）→ `enter_shot_edit` / `confirm_shot_edit` / `cancel_shot_edit`
  （四条出口 —— `Enter` / `Esc` / 工具栏「保存」「退出」—— 汇到同一对收尾函数）；
  `edit_drag`（走 `feed` 的全局钩子事件：边框带上的 Down/Move/Up 被会话吃掉、**框内照常放行**
  给画笔）；`apply_zone` / `sel_rect` / `clamp_sel` 三个自由函数（`apply_zone` 与 GIF 选框
  共用同一套拖拽数学）；热键取**按下那一刻**的 Ctrl 变体（`with_ctrl`），
  `action_hotkey_wakes` 同步列上（否则 park 态按住 Ctrl 截图毫无反应）。
- **`runtime/overlay.rs`**：`zone_at_rt` 扩成两档（GIF 选框 / 编辑态选区）；
  `on_nchit` 编辑态**整屏回 `Client`**（模态编辑，压暗区点击不该穿给下层程序）；
  `draw_gif_frame` 加 `FrameAccent{Select/Recording/Editing}`（同源几何，只换语义色，
  编辑态 = 琥珀）；编辑态放大镜走 `RegionMagnifier::update(&冻结帧, ..)`（**不是**实时抓屏）；
  `shot_edit_active()` 并入"全屏避让"的 `set_suppress_yield`。
- **`modules/drawing`**：`DrawMode::ShotEdit` + `start_with_backdrop`（底图可注入 =
  "截图与标注同一个函数"的代码形态）+ `EditExit{Confirm,Cancel}` + `set_region`（选区外压暗
  与工具栏锚点读它）+ `request_exit`（幂等）+ `toolbar_hit`；`toolbar::layout/hit/draw`
  第三参 `anchor: Option<Rect>`（跟随选区：中心对齐 + 优先下方、放不下翻上方、再放不下退回
  屏幕底边，横纵都钳进屏幕）。

### 关键判据（详见 `开发指引.md` §12.21）

1. **不用独立窗口**：编辑态靠 `on_nchit` 的逐像素判据（"编辑态 ⇒ 整屏 `Client`"）。补丁 083-d
   的 `nchit_at` 优先级最高，绕过 `is_frameless` 缩放边判定，故三态 `MouseMode` 无需第四态。
2. **编辑态不续帧**：屏面静止时一个像素都不变，续帧 = 整屏 CPU 空转（`frame_demand` 的
   *"标注模式空闲时必须报 Idle"*）。`frame_up` 只用于 `partial` 判据与 `invalidate_spotlight`，
   末尾那条 `request_repaint_in` 仍只归 GIF。
3. **工具栏优先于"拖选区"**：选区几乎铺满屏幕时工具条退回屏幕底边，会与选区下边框带重叠 ——
   `edit_drag` 先问 `toolbar_hit`，点在按钮上就放行（否则按钮画着却点不动）。
4. **面板挂起靠既有边沿兜住**：`is_drawing()` 由 false 变 true → `panel_close_hint` → 自动收
   控制面板。`ShotEdit` 档落在 `is_drawing()` 里，这条自动成立。

### 验收（硬证据）

- `cargo test --release`：**826 passed / 0 failed / 4 ignored**（lib）+ 集成 `4+4+2` 全绿。
  本功能新增的门禁分布：`shot_edit` 7 条（产物像素）、`snap` 2 条（`ShotEdit` 是唯一的编辑
  模式 + 取整帧/会话收尾）、`gif_zone` 2 条（`hit_zone_at` 真乘 dpi、边框带比边框厚）、
  `runtime` 5 条（会话进出 / 拖拽吃事件的边界 / 钳位 / `clamp_sel` / 工具栏优先）、
  `toolbar` 5 条（跟随选区：下方 / 翻上方 / 铺满兜底 / 贴边钳位 / `hit` 与 `layout` 同锚点）。
- `cargo fmt --all` 通过；`cargo build --release` mtime `2026-09-23 01:46:57` **晚于**
  最后一次源码编辑 `01:43:55`；`tools/deploy.py` → `MD5_MATCH=True SIZE_MATCH=True`
  （`5264896` 字节），`sync_assets` 不一致 0。

## 补丁 085（**app 侧**，无需改 windui）：贴边不再内缩 / 「截图并贴图」的 Ctrl / 保存=另存为 / Enter 让给文字窗

日期：2026-09-23。四条用户反馈一次收口（逐条见 `开发指引.md` §12.22）。

### ① 智能选区贴合：`EXTENDED_FRAME_BOUNDS` **原样**取（推翻补丁 084 期间的同批决定）

用户两次实测把方向钉死在两个极端：「不内缩 = 边框线向外多 1px」「内缩 1px = 边框线向内少 1px，
连边框线都没有」—— 两句指着同一圈像素。那圈像素**就是** DWM 画给窗口的边框线（半透明，
所以看起来像背景色），正解是照 `EXT` 取。

- `platform/window_probe.rs`：`candidate_of` 去掉内缩；`visible_border_thickness()` 与
  `inset_rect()` **整函数删除**，`DWMWA_VISIBLE_FRAME_BORDER_THICKNESS` 导入一并去掉
  （留着就是下次"顺手用回来"的种子）。模块头那段"必须内缩"的论证换成实测叙述 + 代价说明
  （产物最外圈 1px 会透出桌面色，那是边框本身的性质）。
- **补真窗口实测**（换掉一条假绿）：`the_visible_frame_is_used_as_is` 只喂 `clip_to_desktop`，
  捉不到"在它之前再插一次内缩"。新增 `candidate_rect_equals_the_extended_frame_verbatim`：
  自建 `WS_OVERLAPPEDWINDOW` 真窗口 → 走完整条 `candidate_of` → 自毁。实测
  `GetWindowRect (120,120,360,240)` / `EXT (127,120,346,233)` / 推荐框 `(127,120,346,233)`
  → **四边差全 0**，而差值 `L7 T0 R7 B7` 与 §12.19 的另一组实验互证。不注入输入、不需要独占机器。

### ② 「截图并贴图」按住 Ctrl 没效果：**不是另写的函数，是出厂值把入口堵死了**

`core::hotkeys::with_ctrl` 对**已含 Ctrl** 的规格返回空串（"再按住 Ctrl"物理上不存在另一种按法），
而出厂 `hotkey_pin = Ctrl+Alt+Q` 正是这种规格 ⇒ 它永远进不了编辑态；截图那把（`Alt+S`）不含 Ctrl，
所以一切正常 —— 用户感受到的"不对称"由此而来。两把键其实一直是**同一条** `SnapMode::ShotEdit`，
只在 `pin` 上分岔。

- `core/config.rs`：出厂 `hotkey_pin` 改 **`Alt+Q`**（本体 = 一键直达，变体 `Ctrl+Alt+Q` = 编辑态，
  顺手把老键位还给老用户）。存量配置**不迁移**，只记日志。
- `runtime/mod.rs`：`handle_action_key` 里那串 `if` 抽成纯函数 **`resolve_snap_hotkey()`**，
  规则写进它的文档；顺序改为 **本体优先**（撞弦时"用户按下的是配置里写的那把键"，
  改成变体优先会静默换掉一把键的语义 —— 这正是两条反馈同时错位的机制）。
  新增 `SnapHotkey` 枚举 + `snap_hotkey_truth_table` 锁真相表。
- `core/config.rs`：`migrate_snap_hotkey_shadowing()` 拆开撞弦配置（`hotkey == with_ctrl(hotkey_pin)`，
  例 `Ctrl+F3` + `F3`）＋ `snap_keys_missing_ctrl_variant()` 在 `Config::load()` 里点名
  `log::warn!` —— 让"没有 Ctrl 变体"这件事不再静默。

### ③ 保存 = 另存为；退出 = 默认位置留档；六个出口汇到一处

- `EditExit{Confirm,Cancel}` → **`DrawRequest`** 四变体（`ShotEditConfirm{save_as}` /
  `ShotEditCancel` / `AnnotateSaveAs` / `AnnotateExit`）：控制器**只报意图、一律不动状态**，
  由 `Runtime::handle_draw_request()` 帧内消费。六条出口同一个分派点，所以"按 Enter"与
  "点按钮"在产物上不可能有差别。
- **删掉整条抓屏导出**（`hide_chrome` / `save_stage 1→2→3`）：`hide_chrome` 从某次重构起只写不读
  （工具栏一直被拍进产物），而抓屏拍的是**屏幕**（会连此刻盖在上面的别的窗口一起拍）。
  改**离屏合成** `compose_into`（`Backdrop::paint_into` + 笔迹表，`translate_offset(list, .., 1.0,
  -screen.x, -screen.y)` 1:1 落进 `DibTarget`）：与屏上同源、零等待、不认识别的窗口。
- **产物必须在 `stop()` 之前合成**（`stop_inner` 会把整屏冻结帧还回系统，8 MB/1080p、33 MB/4K）。
- **模态对话框不能就地弹**：`pending_save_as` → `UiMsg::SaveAsReady` → UI 线程 `defer_blocking`
  → `platform::file_dialog::save_shot()`（新增 `TITLE_SAVE_SHOT`）→ `screenshot::save_png`。
  与「贴图 Ctrl+S」同款，避开 windui 铁律 6 的 `&mut` 别名。
- 中央横幅只够一句话：能报"已保存到/已另存为"时用 `stop_quiet()`；没画东西、没得报时仍报
  「已退出绘图」（静默退出会让人怀疑"到底退没退"）。

### ④ `Enter` 与 `Esc` 一样让给文字输入窗

`drawing/mod.rs` 的编辑态分支加 `if !self.text_window_open` 守卫，`NumEnter` 同款 ——
判据复用既有 `text_window_open`，不新造规则。

### 验收（硬证据）

- `cargo fmt --all`；`cargo test --release`：**832 passed / 0 failed / 4 ignored**（lib）+ 集成
  `4+4+2` 全绿。
- **8 条既有测试跟着改契约**（"模式变成 Off" ⇒ "报出意图"）：`controller::debounce`、
  `exit_button_leaves_drawing_mode_and_resets_the_gesture`、`drawing::esc_exits_*` 4 条、
  `runtime::exit_toast_keeps_frames_until_it_fades`、`config_pipeline_e2e::esc_exits_and_reenter_*`。
  改法是断言 `take_request()`，真停那一半由 Runtime 侧测试覆盖。
- 新增：`snap_hotkey_truth_table`、`factory_snap_hotkeys_can_hold_ctrl`、
  `migrate_snap_hotkey_shadowing_splits_the_clash` / `..._keeps_healthy_configs`、
  `missing_ctrl_variant_names_the_offending_keys`、
  `enter_while_text_window_open_is_left_to_the_input_window`、
  `save_button_requests_save_as_in_both_modes`、`candidate_rect_equals_the_extended_frame_verbatim`。
- `cargo build --release` → `tools/deploy.py`：`MD5_MATCH=True SIZE_MATCH=True`（`5268480` 字节），
  `sync_assets` 不一致 0。
- **未覆盖**：① "视觉上刚好贴合"要靠真机再确认（像素判据只能证明"取的是 EXT"）；
  ③ 原生存盘对话框真的弹出、落点正确，属真机项（需要独占机器的按键注入）。


---

## 补丁 086（**app 侧**，无需改 windui）：贴边 1px 的第三种解法（边框线实心化）+ 编辑态 Ctrl 改看"框选那一刻"

两条反馈（用户原话）：

```text
① 截图智能识别窗口与选区，依然多了 1px。我特地截图放大确认过了。
② 我"截图并贴图"，在选区时按住 Ctrl，依然无法在选区后进行调整，而是直接贴图了…
   此时我用 Alt+S 进行单纯的截图，按住 Ctrl 选区后也无法调整。
   换句话说，连同我们原来的选区后的缩放/移动/编辑功能都消失了。
```

### ② 功能回归（先修）：进编辑态的 Ctrl 判定点错了

§12.21/补丁 084 的契约是"`Ctrl` + 截图热键 → 进编辑态"，判定在 `handle_action_key`
（**按下热键那一刻**）。用户的实际操作是"**触发热键之后**，拖着框时按住 Ctrl" —— 两个时刻
被他当成一件事，而代码只认前者。表现就是"按住 Ctrl 毫无效果"，加上两个方向都试过，
看起来像整个编辑功能被删掉了。

改法：判定点移到**框选确认（松手）那一刻**，并且**两边任一成立即可**：

- `runtime/mod.rs`：新增纯函数 `shot_edit_entry(mode, ctrl)`，`confirm_snap` 里现取
  `platform::input::keyboard::ctrl_held()`（全局键盘钩子维护的真实键态）。
  `SnapMode::ShotEdit { pin }`（热键时就带着 Ctrl）与 `SnapMode::Shot|ShotPin` + `ctrl`
  （框选时按住）都进编辑态；GIF 与其余状态恒 `None`。
- 抽成纯函数是为了**可测**：`ctrl_held()` 读全局静态，单测造不出来。
- **2 条既有单测跟着改契约**（`shot_edit_entry` 的"`Shot` + ctrl 不进编辑态"整条反转）。

### ① 那 1px：A / B 之外还有 C

第三次报同一件事（§12.19 内缩 0 → §12.20 内缩 1px → §12.22 回到内缩 0）。先把物理事实钉死
（`temp/probe_grab_alpha.py`，**与产品相同的 GDI 路径**）：

```text
x=1110 外侧  RGBA=(0,0,0,255)       ← 桌面
x=1111 EXT   RGBA=(47,47,47,255)    ← 边框那 1px，alpha 恒 255（GDI 已合成）
x=1112 内侧  RGBA=(248,253,255,255) ← 客户区
反解 (47 - 0.6*0) / 0.4 = 117.5 = #767676（Windows 默认窗口边框色）
```

三方案实测（`temp/fit_border_compare.py`，产物**最外圈单列均值**）：

| 方案 | 最外圈 | 用户的话 |
|---|---|---|
| A `EXT 原样` | `(78, 66, 71)` 混桌面杂色 | "多了 1px" |
| B `EXT 内缩 1px` | `(239, 244, 246)` 客户区 | "连边框线都没有" |
| **C 边框线实心化** | **`(118, 118, 118)`** | ← 唯一自洽解 |

**C 的算法**：`border = (obs - (1 - alpha) * bg) / alpha`，`alpha = 0.4`，
`bg` 取该像素**外侧紧邻**的桌面像素（在冻结帧里）。哪条边贴帧边界就拿不到 `bg` → 原样保留。

改动：

- `core/image.rs`：新增 `RgbaBitmap::crop_frame_opaque(x, y, w, h, alpha)` + 私有
  `rgb_at`。`alpha` 由调用方给（core 只认像素货币，不知道 DWM）。
- `platform/window_probe.rs`：新增 `pub const FRAME_BORDER_ALPHA: f32 = 0.4`（附全部实测）与
  `pub fn rect_is_window_frame(want) -> bool`（枚举 + 逐值比对，**排除本进程**）。
- `runtime/snap.rs::confirm` 加 `screen` 参数（4 处测试调用点同步补 `vs()`）；
  `runtime/shot_edit.rs::compose` 同款判定；`runtime/mod.rs` 一处调用点。

★ 三条纪律（都写进了 `rect_is_window_frame` 的文档）：

1. **逐值相等，不许容差** —— 有容差就会去改不相关截图的边缘。
2. **必须排除本进程的窗** —— 区域选择窗铺满虚拟桌面，截全屏时它的外框恰好等于选区；
   不排除就会把"截全屏"当成"贴窗口边框"。
3. **`.4` 是 Windows 的性质，不是 core 的** —— 常量住 `platform`，core 只收参数。

### 验收（硬证据）

- `cargo fmt --all`；`cargo test --release`：**838 passed / 0 failed / 4 ignored**（lib，上一轮 834）
  \+ 集成 `4+4+2` 全绿。
- **4 条新增**：`crop_frame_opaque_unmixes_the_translucent_border`（**带对照组**：普通裁剪必须
  仍是混色）、`crop_frame_opaque_leaves_edges_without_a_background_sample`、
  `crop_frame_opaque_ignores_degenerate_alpha`、
  `rect_is_window_frame_only_matches_verbatim`（真窗口，正负各一）。
- `cargo build --release` → `tools/deploy.py`：`MD5_MATCH=True SIZE_MATCH=True`（`5270016` 字节），
  `sync_assets` 不一致 0；exe mtime `03:20:29` 晚于最后一次源码编辑 `03:17:04`。
- **未覆盖（真机项）**：① 贴边产物的**视觉**确认；② 框选时按住 Ctrl 是否真的进编辑态；
  ③ 实心化后的边框线在实际窗口上好不好看（`#767676` 是"本色"，而屏幕上看惯的是被桌面
  冲淡后的版本 —— 这是本轮唯一的主观风险点）。

## 补丁 087（**app 侧**，无需改 windui）：贴边的"外圈收进" + 编辑态 `Enter` **彻底无反应**

### 起因（用户原话，两条）

> **1.** 截图时的智能窗口识别，依然会截取到边框线外。我截图并放大观察，发现边框线外的
> 颜色依然被截取了。重复多次验证，依然如此。
>
> **2.** 在截图选区时按下 Ctrl，进入缩放与编辑界面，结果按下 "enter" 键却依然会退出状态。
> 这完全是错的。我已经强调过，按下 enter 键不应该有反应。就如同我们在屏幕标注时，按下
> enter 键也没有反应，这才是对的。按下 enter 键就退出缩放和编辑状态属于严重 bug。

第 1 条是**同一件事又被报了一次**（补丁 085 内缩 0 → 086 内缩 1px → 086 又回到内缩 0 +
边框线实心化 → 本轮）；第 2 条是**上一轮我自己改错** —— 补丁 085 把 `Enter` 做成"没有
文字窗时才确认"，那仍然是"**有反应**"。

### ① 根因不是"内缩几 px"，而是 `EXT` 那一刀在水平方向本身就宽了

**方法：反查真机产物**（本次是决定性的一步，之前三轮都在猜）。
`build/temp/screenshot/CurSeen_2026-09-23_03-25-30.png`（704×608，03:25 —— 晚于 03:20 部署，
有效证据），逐像素读四边：

| 位置 | 实测 | 判读 |
|---|---|---|
| 上边第 0 行 | `(118,118,118)` 整行 | 反解生效、正确 |
| 下边第 607 行 | `(116,118,118)` 整行 | 同上 |
| 左边第 0 列 | `(78,78,85)` / `(85,70,70)` / `(117,81,37)` 随 y 变 | **不是边框** |
| 右边倒数两列 | `(76,76,76)` 恒定 | **不是边框** |

★ 决定性推理：**反解是幂等的**。若外圈真是半透明边框，`unmix(obs, bg)` 必得本色 `118`；
而 `(117,81,37)` 连 `0.6*bg + 0.4*border` 都拼不出来（蓝通道解出 **−17**）→ 那两列
**根本不属于窗户**。

活窗探针（`temp/probe_real_windows.py`，可复跑）：同一个「文本编辑器」窗
（`HwndWrapper[Quicker…]`）此刻 `EXT = (1189,13,707,608)`（宽 **707**），而 03:25 的产物日志是
**704×608** —— 两个数**不相等**，正说明 `EXT` 在水平方向**本身就不稳定**。且它的**左边界恰好
就是"与外侧连成一片"**：`外2=(3,4,4) / 外1=(3,3,3) / EXT=(3,3,5) / 内1..3=(3,3,5)`（差 ≤ 2）；
而上 / 右 / 下三边的 `EXT` 像素与外侧**明显不同**（`(47,47,47)` vs `(0,0,0)`、
`(47,47,47)` vs `(248,248,248)`）→ 判据**有分辨力**，不是"什么都收"。

**结论**：上下都对，水平方向宽了 **左 1px / 右 2px** —— 裁进来的是窗外像素。
★ 已知失效模式：窗户**自身**靠边的几列若恰好与桌面同色（深色窗压深色桌面），会被当"窗外"
收掉 —— **上限 2px** 且肉眼不可分辨；取舍是"宁可收 2px 不可分辨的像素，也不留 3px 明显的
窗外色"。

### ② 解法：`frame_trim` —— 判据是"外圈与外圈之外是不是同一片颜色"

真边界那 1px 是 DWM 半透明边框，与外侧桌面**必不同**（深色桌面上差 40+）；若外圈与外圈之
外几乎一样 → 这一圈落在窗外，收掉。

改动（`core/image.rs`）：

- 新增 `pub struct FrameTrim { left, right, top, bottom }` + `is_empty()` / `total()`；
  `pub const FRAME_TRIM_MAX: u32 = 2`；`const CONTINUATION_TOL: i32 = 2`；
  私有 `enum Side` / `enum Ring { Same, Differs, Unknown }` / `fn near()`。
- 新增 `pub fn frame_trim(&self, x, y, w, h) -> FrameTrim`：四边独立**从外往里**数环，数到
  第一个"与外圈之外不同"的环就停；`fn trim_side` / `fn ring_vs_outside`（每边 **90% 以上**
  像素成立才算 `Same`）。
- `crop_frame_opaque` 开头插入收边（**两趟，顺序不能反**）：
  ① `frame_trim` 收掉不属于窗户的环 → ② 对**收完之后**的最外圈反解
  `border = (obs - 0.6*bg) / 0.4`。

★ 四处**保守**设计（缺一条就会去乱削正常截图）：上限 2px（真机规模就是 1–2px）；90% 阈值
（应对圆角 / 一段边被别的窗压住）；**一次边界都没数到就一点都不动**（纯色图）；
贴帧边界（拿不到"外圈之外"样本）的那条边也不动；收完剩 < 4px 整条作废。

调用点**无需改动**（`runtime/snap.rs::confirm` 与 `runtime/shot_edit.rs::compose` 的
`crop_frame_opaque` 自动吃到新逻辑）。

### ③ `Enter`：连一个分支都不给

`modules/drawing/mod.rs` ⓪' 段**删掉 `Enter`/`NumEnter`**，编辑态只剩 `Esc`；`take_request`
的文档同步（不再提 `Enter`）。编辑态的两个出口：工具栏「保存（另存为）」与 `Esc`（放弃）。

测试 `enter_never_does_anything_in_shot_edit`：两种挂起状态 × `Enter`/`NumEnter` 全不报请求，
**末尾对照组断言 `Esc` 仍报 `ShotEditCancel`**（证明不是"按键链路死了"）。

### ④ 顺手扶正一条**假红**单测

`platform::input::hit::tests::window_from_point_skips_a_click_through_window`：全量跑偶发红、
单跑三次全绿。判据太弱 —— 断言 `!over_our_window(150, 150)`（PID 比对），而 `window_probe`
的用例在 `(虚拟桌面原点 + 120, +120)` 建了一个 360×240 的**非穿透**窗，正好盖住 `(150,150)`，
并行就红。改为比**句柄**（`WindowFromPoint(at) != 替身窗`），并加对照组：不穿透的替身窗
**必须**被取到。

### ⑤ 纪律：**喂给反解的样本，必须能反解回本色**

新夹具第一版把边框随手写成 `(47,200,47)`，反解出来 `(103,255,73)`（就是这样红的）。改成
物理自洽的 `obs = 0.6*bg + 0.4*118 = (53,59,65)`，已写进 `desktop_with_window` 的文档。

### 验收（硬证据）

- `cargo fmt --all`；`cargo test --release`：**841 passed / 0 failed / 4 ignored**（lib，
  上一轮 838）+ 集成 `4+4+2` 全绿。
- **新增 3 条**：`frame_trim_removes_the_ring_that_continues_outside`（**带对照组**：正正好好的
  那一刀一像素都不收；端到端尺寸回 5×4、最外圈 = 118）、
  `frame_trim_leaves_uniform_frames_alone`、`frame_trim_gives_up_on_edges_at_the_frame_border`。
  **改 2 条**：`enter_never_does_anything_in_shot_edit`（替换
  `enter_while_text_window_open_is_left_to_the_input_window`）、
  `window_from_point_skips_a_click_through_window`（判据 + 对照组）。
- `cargo build --release` → `build/deploy.py`：`MD5_MATCH=True SIZE_MATCH=True`
  （`5270528` 字节，上一轮 `5270016`），`sync_assets` 不一致 0；exe mtime `03:55:52` 晚于
  最后一次源码编辑 `03:53:42`。
- **未覆盖（真机项，需独占机器）**：① 收边后产物的**视觉**确认；② `Ctrl` 在框选时按住是否
  真进编辑态；③ `Enter` 按下去确实没反应（本轮只有单测）。

---

## 补丁 088（windui 侧）：托盘图标的运行期切换 —— `TrayHandle::set_icon_rgba`

### Objective
应用在运行期换托盘图标（CurSeen 的用途：录制中把圆点变红，让用户一眼认出"正在录"）。
此前运行期只能换提示（`SetTooltip`），图标在 `NIM_ADD` 之后就焊死了。

### Modified Files

#### `src/platform/tray.rs`
- `TrayOp` 新增变体 `SetIcon { w: u32, h: u32, rgba: Arc<[u8]> }`（非预乘 RGBA8，
  与 `Tray::icon_rgba` / `WindowIcon` 同口径；`Arc` 让 `pending_ops()` 的测试克隆变廉价）。
- `TrayHandle::set_icon_rgba(w, h, rgba)`：入队 + `request_repaint()`（与 `set_tooltip`
  同理：换图标是状态沿，之后用户大概率立刻瞟一眼托盘，不能等下一次鼠标事件）。

#### `src/platform/win32/tray.rs`
- 自由函数 `set_icon(hwnd, uid, hicon) -> bool`：`NIF_ICON` + `NIM_MODIFY`。**只带
  `NIF_ICON`**——`NIM_MODIFY` 只处理 `uFlags` 声明的字段，不会把提示抹掉。
- `TrayState::swap_icon(hicon, owns)`：销毁旧句柄（若自建）并记下新句柄。`readd` /
  `reinstall` 经 `readd_target()` 取的是 `self.hicon`，因此 shell 重启重放自然拿到
  **现在**的图标，不会退回启动时那张。
- 顺序约束（写进两个函数的文档）：`NIM_MODIFY` 返回后 shell 已完成跨进程复制，
  此刻销毁旧 `HICON` 才安全；调用方必须先 OS 调用后记账。

#### `src/platform/win32/mod.rs`
- `apply_tray_ops` 新增 `SetIcon` 臂：建 `HICON`（`hicon_from_rgba`）→ `set_icon` →
  之后才借宿主 `swap_icon`（与 `SetTooltip` 的"先 OS 后借用"同序）。尺寸不符建不出
  句柄 → **静默丢弃 + warn 日志**（调用方编程错误，不能崩消息循环）；`NIM_MODIFY`
  失败（托盘刚被 shell 丢弃）→ 就地 `DestroyIcon` 防泄漏。

#### `src/platform/macos/tray.rs`
- `apply_tray_ops` 补空臂（NSStatusItem 换图要走 NSImage 解码通路，CurSeen 只跑
  Windows），保持 match 穷尽；意图被丢弃，与"没装托盘"同语义。

### 消费方（CurSeen app 侧，不属于本补丁的 windui 改动）
- `assets/icons/logo_red.png`（red 版源图，圆点红色）→ `build/gen_icon.py` → `logo32_red.rgba`。
  ★ 命名跟源图走 `red`，不叫 `rec`——用途（录制期间用）归调用方，文件名只说它是什么颜色。
- `platform::icon::red_logo_rgba()` + `app::tray::set_tray_recording(on)`；
  调用点：`runtime::record` 的开录成功（→红）/ 停止与退出兜底（→常态）。

### 验收（硬证据）
- 新增护栏测试 `platform::icon::red_logo_carries_a_red_dot`：红像素 > 8、
  青色箭头仍在、长度 4096——红版做错（漏图 / 没换色）直接红灯。
- `cargo test --release -- --test-threads=1` 全绿（1013 passed / 0 failed）。

## 补丁 089（windui 侧）：滚动条贴窗右缘的内缩按窗口能力归零

### 背景与现象
`core::scrollbar::WINDOW_EDGE_INSET = 8` 让贴窗口右缘的滚动条整体内缩 8 逻辑 px，
为的是避开无边框窗口 `WM_NCHITTEST` 判成 `HTRIGHT` 的缩放边框（否则"看得见点不着"）。
但补丁 022-a 之后，`resizable(false)` 的窗口（无 `WS_THICKFRAME`）**已经没有缩放热区**
——整窗 `HTCLIENT`，右缘 8px 不吞任何事件。内缩却还是无条件的：控制面板右侧那条
11px 的缝（inset 8 + margin 3）里，有 8px 是白让的，滚动条看起来"没贴到右边"。

### Modified Files

#### `src/core.rs`
- `Tree` 新增字段 `pub scroll_window_edge_inset: i32`（默认 `WINDOW_EDGE_INSET`，
  老宿主行为不变）；`Tree::scrollbar_edge_inset` 改读该字段，`<= 0` 直接返回 0。
  命中区（`scrollbar_hit_zone`）、绘制（paint 侧 `tx`）与 arrange 预留宽
  （`occupied_w(inset)`）都从这一个函数取值，自动同源。
- 新增护栏测试 `scrollbar_edge_inset_is_host_configurable`：置 0 后命中区直达容器
  右缘且宽度不变；默认值保持历史行为。

#### `src/app/mod.rs`
- `App::new` 建树时按窗口能力设置：`cfg.resizable` → `WINDOW_EDGE_INSET`，否则 0。
  这是唯一建"真窗口"树的地方（`run_app_callback` / `drain_app_channels` 的两棵
  detached 树不布局、不画滚动条，不用管）。

### 消费方（CurSeen app 侧）
- 无代码改动：控制面板 `Window::new(..).resizable(false)` 自动获得贴边滚动条。

### 验收（硬证据）
- 真机截图实测（修复前）：面板 thumb 右缘距窗右缘 19 物理 px，扣除 8px 外框后
  客户区内缝 11px = `WINDOW_EDGE_INSET(8) + MARGIN(3)`。
- `cargo test --release -- --test-threads=1` 全绿（1013 passed / 0 failed）。


---

## 补丁 090（windui 侧）：`fast_fill_rect` 半透明混合加「目标全透明直写」

### 背景与现象（0.20.0 迁移时发现）
补丁 031 作废、这条路径交给上游 `fast_fill_rect` 之后，`frame_cost_probe` 实测聚光灯
暗罩的光栅成本**比 0.19-fork 高 3.4–3.7 倍**（同一探针、同一口径）：

| 分辨率 | 0.19-fork | 0.20（补丁 090 前） | 0.20（补丁 090 后） |
|---|---|---|---|
| 2560×1440 spotlight 光栅 | 2.90 ms | 9.80 ms | **2.08 ms** |
| 3840×2160 spotlight 光栅 | 6.13 ms | 22.88 ms | **5.16 ms** |
| 2560×1440 一帧总成本（拖尾+spotlight） | 7.72 ms | 13.72 ms | **6.05 ms** |

**根因**：上游 `fast_fill_rect` 对半透明源**恒做** `p[i] = src[i] + mul255(p[i], ia)`
（四次乘法 + 「先读后写」依赖链）。而 CurSeen 覆盖层每帧都是从「清屏(透明) → 填图元」
开始，目标像素恒为 `alpha == 0` —— 此时 `SourceOver` 退化为 `Source`（目标不贡献任何
颜色），结果就等于源色本身。

### Modified Files

#### `src/render/skia.rs`
- `SkiaCanvas::fast_fill_rect` 的**三处**半透明混合各加一条分支：
  直段（`data[base+lo..base+hi]`）、圆角行中段（`data[base+mlo..base+mhi]`）、
  圆角带（按覆盖率缩放源色那处）。
  ```rust
  if p[3] == 0 { p.copy_from_slice(&src); } else { /* 原混合 */ }
  ```
  语义等价（目标 alpha=0 时混合结果恒等于源色），只省掉乘法与读依赖链。

### 验收（硬证据）
- `cargo test --release -- --test-threads=1` 全绿（含上游 5 条 `fast_fill_rect_*` 用例）。
- `cargo run --release --example frame_cost_probe` 数字见上表；迁移后三种分辨率下
  一帧总成本均**优于** 0.19-fork。

## 补丁 091（windui 侧）：托盘右键菜单朝左上展开，不再被任务栏切掉下半截

### 背景与现象（2026-09-28）
右键点托盘图标弹出的菜单**经常被任务栏遮挡一部分**——菜单下沿那一截看不见。

### 根因
`track_menu` 调 `TrackPopupMenu` 时只给了 `TPM_RIGHTBUTTON | TPM_RETURNCMD`，**没给任何对齐
标志**。此时菜单以光标为**左上角**朝右下生长；而托盘区就在屏幕**底部**（任务栏默认位置），
菜单下沿会越过屏幕边界被裁掉。看起来像"被任务栏挡住"，实质是**自己长到了屏幕外**。

### Modified Files

#### `src/platform/win32/tray.rs`
- `track_menu` 的 `TrackPopupMenu` 标志加 `TPM_BOTTOMALIGN | TPM_RIGHTALIGN`：菜单**下边**
  钉在光标处（朝上长）、**右边**钉在光标处（朝左长），两条一起菜单稳定朝左上展开，落在
  屏幕内侧。这是托盘菜单的通行做法，不改变任何交互语义（`TPM_RETURNCMD` 照旧返回命令 id）。
- 导入相应补 `TPM_BOTTOMALIGN` / `TPM_RIGHTALIGN`。

### 验收（硬证据）
- `cargo clippy --release --tests` 零新告警；`cargo test --release -- --test-threads=1` 全绿。
- **真机复核项**（执行环境无法自动验视觉，交用户）：任务栏在底部时右键托盘图标，菜单应完整
  可见；顺带确认「打开控制面板」「退出」两项可点、点外部菜单照常消失。

## 补丁 092（windui 侧）：数位板"轻触即书写"——关停笔的系统手势

### 背景与现象（2026-09-28）
用户用数位板在屏幕标注里写字，**笔尖轻触时画笔不起效**，必须用力快压才能落墨。
本应用拒绝压感曲线（简易工具定位），问题只能出在事件层。

### 根因
Windows 默认给触控笔配了一整套系统手势，窗口对 `WM_TABLET_QUERYSYSTEMGESTURESTATUS`
不表态时 `DefWindowProcW` 应答**全开**。其中 press-and-hold（长按变右键）会让系统在
笔尖接触后先进入计时窗、等"要不要变右键"——起笔被吞，轻触就写不出第一笔。

### 修法
`wnd_proc` 增加一条臂，对所有窗口统一应答 `TABLET_GESTURE_DISABLE_MASK`：关停
press-and-hold / 点按涟漪 / 笔杆涟漪 / 触屏开关 / flick 及其回退键，**保留墨迹视觉
反馈**（`TABLET_ENABLE_PENSTROKEFEEDBACK`，纯视觉不吞事件）。本应用没有需要这些
手势的窗口（右键菜单由真实右键承担），代价为零。

### Modified Files

#### `src/platform/win32/mod.rs`
- 常量 `WM_TABLET_QUERYSYSTEMGESTURESTATUS = 0x02C0` 与
  `TABLET_GESTURE_DISABLE_MASK`（逐位注释）。
- `wnd_proc` 在 `WM_MOUSEMOVE` 前新增 `WM_TABLET_QUERYSYSTEMGESTURESTATUS` 臂。

### 验收（硬证据）
- `cargo build --release` 过；全量测试绿。
- **真机复核项**（执行环境无数位板，交用户）：数位板笔尖**轻触**屏幕标注即出墨，
  无需用力；右键笔键照常。

## 补丁 093（windui 侧）：笔的 WM_POINTER 直通——根治起笔 0.5 秒延迟

### 背景与现象（2026-09-28）
补丁 092 关停系统手势后，用户二报：**每次落笔的前 ~0.5 秒按压/走动的痕迹仍延迟出现**，
之后才与笔同步。说明"指针→传统鼠标消息"的翻译管线前仍有消歧延迟，应用侧掩码压不到零。

### 修法
`wnd_proc` 直接收 `WM_POINTER_DOWN/UPDATE/UP`（笔尖一触系统即刻送达，不经翻译）：
`GetPointerType` 只接管 `PT_PEN`（触摸/鼠标管线分毫不动），`GetPointerInfo` 的
`ptPixelLocation` 经 `ScreenToClient` 后喂给既有的 `handle_pointer_at`；笔尖 = 左键、
笔杆键 = 右键（悬停期做边沿检测）。笔接触期间吞掉系统补发的翻译 `WM_MOUSE*`，
防止一笔画两遍。悬停期 UPDATE 仍喂 Move，悬停高亮不丢。

### Modified Files

#### `Cargo.toml`
- windows features 增 `"Win32_UI_Input_Pointer"`。

#### `src/platform/win32/mod.rs`
- 常量 `WM_POINTER_UPDATE/DOWN/UP`（0x0245/0x0246/0x0247）。
- 静态 `PEN_CONTACT`（接触期吞翻译鼠标消息；取 info 失败也复位，防永久失效）、
  `PEN_LAST_FLAGS`（笔杆键边沿检测）。
- `wnd_proc`：新增 pointer 三臂；`WM_MOUSEMOVE/WM_LBUTTON*/WM_RBUTTON*` 在
  `PEN_CONTACT` 时直接吞掉。
- 新函数 `handle_pen_pointer`（坐标/按钮换算 + 分发）。

### 验收（硬证据）
- `cargo check --release --tests` 过；全量测试绿（见主仓门禁）。
- **真机复核项**（执行环境无数位板，交用户）：落笔**即时**出墨且轨迹与笔同步；
  笔杆键右键照常；鼠标/触摸输入行为不变。


> 说明：本项目 fork 的 `layered` feature（补丁 001）、以及全部补丁标记都在本表登记；`tests/docs_consistency.rs::test_patch_record_has_a_section_for_every_windui_patch` 是门禁。

## 补丁 093 延伸：指针来源类型暴露给宿主——笔直通落到绘制链路

### 背景（2026-09-28 用户三报）
补丁 093 已把笔的 `WM_POINTER` 换算成 windui 指针事件，但宿主的书写输入**只吃
`WH_MOUSE_LL` 钩子**（`modules/drawing` 由 `InputEvent` 驱动），窗口事件那条路
对绘制毫无作用 —— 直通白接了，起笔延迟原样。

### 修法（宿主侧改动见主仓 `runtime/overlay.rs` / `platform/input/mouse.rs`）
windui 补上这条指针事件来自什么设备这一缺失信息：`PointerEvent` 本身不带设备类型，
而宿主必须**只对笔**走直通、鼠标照旧走钩子 —— 两条路同时喂会把一笔画两遍。
- 线程局部 `CUR_POINTER_KIND`：`handle_pen_pointer` 置 `PT_PEN`、`handle_pointer`
  置 `PT_MOUSE`。宿主 `on_event` 在 `dispatch_pointer_event` 里**同步**被调，
  读到的一定是本条事件自己的值，不用清理。
- `pub fn pointer_is_pen() -> bool`（`platform::mod` 转出；非 Windows 恒 false）。

### Modified Files

#### `src/platform/win32/mod.rs`
- 线程局部 `CUR_POINTER_KIND`（i32，存 `POINTER_INPUT_TYPE` 原始值）。
- `handle_pointer` / `handle_pen_pointer` 各置一次。
- 新函数 `pointer_is_pen`。

#### `src/platform/mod.rs`
- `pub use win32::pointer_is_pen;`（windows）+ 非 Windows 兜底 `false`。

### 验收（硬证据）
- `cargo check --release --tests` 过；全量测试绿（见主仓门禁）。
- **真机复核项**（执行环境无数位板，交用户）：落笔即时出墨、前 0.5 秒不再延迟；
  鼠标书写完全不受影响（`PEN_ACTIVE` 恒 false 时钩子零改动）。

## 补丁 094（windui 侧）：`Clickable` 失焦回调 `on_blur`——快捷键框点别处即退出捕获态

### 背景（2026-10-02 用户报告）
快捷键捕获框点下后进入捕获态（「按下按键…」），此时**点面板上任何其它地方**，捕获态
永远挂着：框里一直等人按键、原快捷键已被点击时清掉，框处于"半死"状态。用户口径：
失焦就该像网页输入框一样**直接为空**。

根因：windui 的焦点变更（`Tree::set_focused`）只翻节点标志位，**不向控件发任何事件**；
点空白处清焦点（`apply_dispatch_effects` 的 blur 分支）控件同样无感。控件的失焦感知
此前只有 stepper 那条路——在 `paint` 里读 `focused` 参数做"失焦提交"。

### 修法
给 `Clickable` 补上可选的失焦回调，走同一机制：paint 时检测 `focused` 的 true→false
边沿即触发。回调类型 `BlurFn = Rc<dyn Fn()>`——paint 阶段拿不到 `EventCtx`，回调只该
做"改信号"这类纯状态动作。

### Modified Files

#### `src/core.rs`
- 新回调类型 `pub type BlurFn = Rc<dyn Fn()>`（注释说明为何无 ctx）。
- `Widget` trait 新增默认方法 `take_blur`（默认无操作，仅 `Clickable` 实现）。

#### `src/ui/containers.rs`
- `Clickable` 新增 `on_blur: Option<BlurFn>` 与 `had_focus: Cell<bool>`（边沿检测）。
- `paint` 开头：上一帧有焦点、这一帧没有 → 触发 `on_blur`。
- `Widget::take_blur` 实现。

#### `src/ui/mod.rs`
- `Element` 新增 `blur` 字段 + 链式构建器 `on_blur(impl Fn())`。
- `build` 时经 `take_blur` 注入控件（与 `click` 同一条路由）。

### 宿主侧消费（主仓 `src/ui/card.rs`）
`hotkey_row` / `hotkey_row_script`（快捷键捕获框）挂 `on_blur`：失焦时若捕获目标仍是
本框则 `cap.set(None)` → 显示回「（空）」。**必须判"目标仍是本框"**：点另一个捕获框时
本框的 blur 在 paint 时才触发，可能晚于新框 `on_click` 置上的新捕获态，无脑清会把它打掉。

### 验收（硬证据）
- `cargo check --release` 过（windui + 主仓）；主仓全量测试绿（见主仓门禁）。
- **真机复核项**（交用户）：点快捷键框 → 点面板空白/其它控件，框立即显示「（空）」；
  点另一个快捷键框则捕获态正确切到新框，不被旧框的 blur 打掉。


---

## 补丁 095（windui 侧）：`Window::popup()`——贴图窗建窗即 `WS_POPUP`，解除系统跟踪尺寸收口

### 背景（2026-10-24 用户报告）
用户原话：*"贴图，长图，放大之后，图片并没有放大，只是外部的边框在放大。可以试着复制
很多行文字再贴图。"* 并复现确认：*"对于宽高比极端的图，我进行放大，实际上图片没有真的
放大。如果复制很多行文字再贴图，就能看到类似的问题。因为多行短文字就会变成特别长的图，
一下就能看出来。"*

### 根因（两条各自独立，都已修）
1. **布局陈旧**（早已修）：贴图窗被 `on_wheel` 用 `SetWindowPos` 外部缩放，windui 的
   `WM_SIZE` 不请求重排 → `paint` 拿到的 `bounds` 停在旧尺寸。`live_bounds` 拿
   `GetWindowRect` 真尺寸作画治这一条。
2. **系统跟踪尺寸收口**（本补丁）：固定大小 + `frameless` 的窗样式仍是
   `WS_OVERLAPPEDWINDOW & !(WS_THICKFRAME|WS_MAXIMIZEBOX)` 一族，`CreateWindowExW` 与
   `SetWindowPos` 都按 `WM_GETMINMAXINFO` 的跟踪尺寸收口：
   - **宽硬下限** `SM_CXMINTRACK`（本机实测 136）。`ptMinTrackSize` 写 1 也压不到更低
     （实测：写 (1,1) 仍得 136；写 200 才变 200）——这是 Windows 对非 popup 样式的硬下限。
   - **高上限**约一屏（本机实测 1100），来自 `DefWindowProcW` 预填的 `ptMaxTrackSize`
     （实测：接管 `WM_GETMINMAXINFO` 只写 `ptMinTrackSize`、不调 `DefWindowProcW` 时，
     高仍被钳到 1100，证明默认值就够小；显式写 `ptMaxTrackSize=16384` 才解除）。

   于是 60×720 的窄长条（多行短文字栅格化出来的，12:1）窗口被顶到 136 宽，`fit_aspect`
   把图居中、两侧各 38px 黑边；长图放大到窗高越过 1100 后**图片高度被钉死**，而窗口另一维
   还在按百分比长——图片尺寸冻结、外框继续变大，正是用户看到的现象。

   候选修法一一排掉：`.min_size(1,1)` 无效（见上，136 是硬下限）；**只有换样式为
   `WS_POPUP` 有效**。

### 修法
`WindowRequest` / `WindowConfig` 新增 `popup: bool` + `Window::popup(on)` 构建器；
`create_window` 里 `if cfg.popup { win_style = WS_POPUP; }`（与补丁 004 的 `layered`
同一位置、同语义但独立开关——调用方要的是"窗口尺寸精确等于内容"，不要分层那套透明/置顶
副作用）。`frameless_like` 一并纳入 `cfg.popup`（`WS_POPUP` 本无非客户区，按带框反算会
凭空加出一个不存在的边框余量）。

主仓 `src/runtime/layer/pin.rs::open()` 加 `.popup(true)`。

### 为什么换了样式其余 frameless 机制不受影响
三者都不看样式位：
- `WM_NCCALCSIZE`：`is_frameless(hwnd)` 读**宿主状态**（`cfg.frameless`），非最大化时
  `rgrc[0]` 不动 → 客户区 = 整窗。
- `WM_NCHITTEST`：缩放热区判据是 `WS_THICKFRAME`，新旧样式都没有它 → 都不是热区。
- `WM_GETMINMAXINFO`：本窗 `min_w/min_h` 恒 0，走到 `DefWindowProcW` 时 popup 无收口。

### Modified Files
#### `src/event.rs`
- `WindowRequest` 新增 `pub popup: bool`（带 `★ CurSeen 补丁 095` 标注与分工说明）。

#### `src/app/mod.rs`
- `Window::new` 初始化 `popup: false`；`App::new` 的 `WindowConfig` 同样补 `popup: false`。
- 新增构建器 `Window::popup(mut self, on: bool)`。
- `build_new_window` 显式传递 `popup: req.popup`（**必须显式**：`..WindowConfig::default()`
  会把漏掉的字段一律填成 false，漏一个就是"设了 popup 但窗口还是被收口"，与补丁 018 的
  `pos` 同一类坑）。

#### `src/platform/mod.rs`
- `WindowConfig` 新增 `pub popup: bool` + `Default` 里 `popup: false`。

#### `src/platform/win32/mod.rs`
- `create_window`：`cfg.layered` 之后加 `if cfg.popup { win_style = WS_POPUP; }`。
- `frameless_like` 纳入 `cfg.popup`。

### 验收（硬证据）
- `cargo check --release` / `cargo clippy --release`（windui + 主仓）过。
- **平台级探针** `tests/probes/pin_zoom_clamp_probe.rs`（四变体对照）：
  | 变体 | 做法 | 结果 |
  |---|---|---|
  | 0 | 默认（`DefWindowProcW`） | 要 64×724 → 136×724；要 64×3004 → 136×1100 ★被钳 |
  | 1 | 接管 `WM_GETMINMAXINFO`，min=200/max=16384 | 要 64×724 → 200×724（min 生效、高不再钳） |
  | 2 | **只写 min=(1,1)**，不碰 max、不调 `DefWindowProcW`（= `.min_size(1,1)` 的行为） | 与变体 0 完全一致，仍被钳 ⇒ `.min_size` 这条路无效 |
  | 3 | **建窗后换样式 `WS_POPUP`** | 2×80 / 3000×80 / 60×4000 **全部原样授予** |
- **端到端探针** `tests/probes/pin_zoom_e2e_probe.rs`（起真实 exe、剪贴板放 60×720 纯品红
  CF_DIB、`SendInput` 发 Ctrl+Alt+P、按标题找窗、截屏量品红包围盒）：
  - **修后**：`style=0x94000000`（`WS_POPUP|WS_VISIBLE|WS_CLIPSIBLINGS`），出厂窗口
    **64×724 == 图 60×720 + 2×FRAME**，客户区 == 外框，品红包围盒 **(2,2,60,720) 铺满无黑边**；
    滚 3 格后窗 82×940、品红 (2,2,78,936)，**都 ×1.30**。
  - **判据能自己红一次**（项目纪律）：临时撤掉 `.popup(true)` 重跑 → 出厂断言当场红，
    报"要 64x724，实际 136x724、style=0x14c80000"，与用户报的现象逐位一致。
- 主仓全量测试绿（见主仓门禁）。

### 宿主侧消费（主仓 `src/runtime/layer/pin.rs`）
`open()` 的窗口链加 `.popup(true)`，注释写明"删了就是极端宽高比贴图图片不缩放复发"。
`pin.rs::tests` 新增 `extreme_aspect_ratio_window_is_exactly_image_plus_frame`：钉住
"窗口 = 图片 × 缩放 + 2×FRAME"的算式（含 800% 上限 484×5764 远超一屏高也不许被钳），
与真机探针分工——单测管算式、探针管"算式真的落到窗口上"。
