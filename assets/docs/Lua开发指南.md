# Lua 脚本开发指南

> 本指南讲**怎么写**一个 CurSeen Lua 脚本：放哪、结构、UI 参数、生命周期、调试与发布。
> 逐条接口（每个函数 / 字段 / 返回值）查 [`../lua/API.md`](../lua/API.md)（权威表，与代码同源）；
> 工具与接入方式查 [`接入指南.md`](接入指南.md)；按功能找规则查 [`README.md`](README.md) 的主题表。
> 本文件随包分发，AI 可用 `read_doc` 直读。

## 1. 先决定放哪：五个挂载目录

一个脚本就是一个 `.lua` 文件，放进 `assets/lua/` 五个挂载目录之一。**目录决定用户在哪选到它**——放错，脚本再好也不出现：

| 目录 | 用户在哪看到 | 脚本形态 |
|---|---|---|
| `assets/lua/MouseTrail/` | 拖尾特效下拉 | 轨迹类：`on_move` + `step` + `draw` |
| `assets/lua/HoverDot/` | 悬浮标类型下拉 | 跟随类：`draw` 每帧画一次 |
| `assets/lua/ClickFx/` | 点击效果下拉 | `on_press` 触发一次，之后自行淡出停止 |
| `assets/lua/KeySound/` | 击键音效 | `run(ctx, params)`，按键触发一次，不是逐帧绘制 |
| `assets/lua/scripts/` | 「脚本」页常驻卡片（`category` 分组排序） | 复合脚本：可绑全局热键 |

- **五个挂载都热重载**：脚本文件改完立刻重建实例；`KeySound/` 在下一次按键生效。面板上的卡片名/参数格属于配置外形，可能要到下次开面板才刷新。
- 显示名用顶层全局 `effect_name` / `name`（互为别名）；一句话说明用 `description`。
- 类别与角色等顶层全局（`kind` / `role` / `menu` / `requires` / `category`）见 API.md §7。

## 2. 最小脚本（真实出厂例：`HoverDot/solid.lua`）

```lua
effect_name = "实心圆"
name = "实心圆"
role = "effect"

function schema()
  return {
    { key = "diameter", label = "直径", type = "int",   min = 4, max = 1000, step = 1, default = 50 },
    { key = "color",    label = "颜色", type = "color", alpha = true, default = "#FF000040" },
  }
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#FF000040")
  local d = params.diameter or 50
  ctx:circle(mouse_x, mouse_y, d / 2, r, g, b, a)
end
```

三段各司其职：

- **顶层全局**（`effect_name` / `role`…）：宿主扫描期读取，决定显示名与宿主。
- **`schema()`**：声明参数 → 面板自动生成控件（下一节）。缺了就没有参数区，不算错，但建议写。
- **回调**（这里只有 `draw`）：宿主在合适时机反向调用；缺失的回调静默跳过（`schema` / `init` 除外）。

★ 三条判活铁律（`--lua-trace` 会自动判，不过就是不过）：

1. `draw` **第 0 帧必须产出指令**——宿主按"这一帧画不画"判活，首帧不画 = 实例当场判死。
2. 悬浮标/复合脚本：指针停住后若还有动画（弹簧回摆、平滑追赶、淡出），`draw` 必须返回一个
   正数（毫秒，如 `16`）继续要帧；收敛后必须**不再返回**，否则静止态永久烧 CPU。
3. 点击效果必须**自己停笔**（返回前先判寿命），否则实例永不释放。

## 3. UI：`schema()` 参数表

`schema()` 返回数组，每项一个参数；面板按 `type` 生成控件。逐字段语义以 API.md §9 为准，常用项：

| 字段 | 说明 |
|---|---|
| `key` | 配置键，脚本里 `params.<key>` 取用（声明了就必须真的读） |
| `label` | UI 显示名；缺省 = `key` |
| `type` | 控件类型：`float`（滑条+输入框）/ `int` / `bool` / `color`（取色器）/ `choice`（下拉，配 `choices`、可选 `labels`）/ `sound` / `folder` / `file` / `str` / `hotkey` 等；未知类型按 `float` 处理 |
| `min` / `max` / `step` | 滑条范围与步进（非数值类忽略） |
| `default` | 默认值；界面初值与脚本默认同源 |
| `alpha` | 仅 `color`：默认 `true`（8 位 `#RRGGBBAA`，`hex_to_rgb` 第 4 返回值是 0–255 alpha） |
| `group` | 可选分组标题，参数多时把 UI 分组 |

★ **最容易踩的坑：`default` 不会自动进 `params`。** 只有 `image` 角色的 `run` 模型会合并
schema 默认值；拖尾 / 悬浮标 / 点击 / 复合、击键音效、以及离屏工具（`--lua-trace` / MCP `params`）
吃的都是**裸配置**。所以每个 `params.<key> or <V>` 的兜底值 `V` 必须与 `schema()` 的
`default` **逐项一致**，否则"离屏过了、真机不对"或"改了默认值画面没变化"。详见
[`MCP说明.md`](MCP说明.md) §5 与 API.md §8。

参数会被面板裁剪：给哪个特效设参数，就得同时把它选成当前特效（工具 `set_active` 会一起做）。

## 4. 生命周期与四类角色

`role` 决定宿主形态（详见 API.md §8 / §10）：

| 角色 | 目录 | 入口 | 备注 |
|---|---|---|---|
| `effect` | MouseTrail / HoverDot / ClickFx | 回调：`on_move` / `step` / `draw` / `on_press` / `on_mouse_down` / `on_mouse_up` / `on_wheel` | `dt` 单位 = **秒**；老化 / 淡出放 `step`（鼠标不动也要推进） |
| `composite` | scripts/ | 同上 + `on_key` | 可绑全局热键；专属 `play_sound()` |
| `image` | 任意（贴图右键动作） | `run(ctx, params)` | `ctx.image` 拿到贴图；结果用 `ext:result()` 开窗 |
| `keysound` | KeySound/ | `run(ctx, params)` | `ctx.key` / `ctx.mods` / `ctx.sounds`；发声要显式调 |

两个都叫 `ctx` 的东西别混用（API.md §0）：

- **绘图模型** `draw(ctx, params)` 的 `ctx` 是 userdata，冒号调用发指令（`ctx:circle(...)`）；
  指令全集（圆 / 环 / 矩形 / 多边形 / 文字 / 渐变 / 辉光 / 桌面采样镜片…）见 API.md §3。
- **运行模型** `run(ctx, params)` 的 `ctx` 是 table（`ctx.role` / `ctx.fs` / `ctx.image`…）。

时间用哪个全局：逐帧动画相位用 `elapsed`（覆盖层休眠时不前进）；"距某事件过了多久"、
静止检测 / 超时淡出用 `wall_ms`（随真实时间前进）。指针 / 屏幕环境变量清单见 API.md §2；
悬浮标的坐标系与续帧细节见 `../lua/HoverDot/README.md`。

## 5. 能力边界：`ext`、`ctx.fs` 与数据落点

- `ctx.fs`（仅 run 模型）：**只读**，根 = 脚本目录；绝对路径 / `..` 一律拒——这是防手滑纪律，不是沙箱。
- 全局 `ext` 所有脚本可用（API.md §4）：进程（`proc`，只准起脚本目录里的 `.exe`）、
  HTTP / 下载（`http` / `download`）、ZIP 解包（`unzip`）、对话框（`input` / `open_file` /
  `save_file`）、文本窗（`result`）、AI 对话窗（`chat_window`）、图片对象（`load_image` /
  `ExtImage`：pixel / bytes / crop / rotate / flip / save）、剪贴板（`clip_get` / `clip_image` /
  `clip_set`）、JSON、任意路径读写（`read_file` / `write_file`）。
- ★ `ext:http` / `proc` / `download` / `input` / `chat_window` 会**阻塞**，只能在 `run`
  模型（工作线程）里调；`draw` 逐帧跑在 UI 线程，调用会被拒绝或自锁。
- 落点三处：临时文件 `ext:tmpdir()`；持久缓存 `ext:data_dir()`（不在 assets 下、不触发
  目录监听、不被工作文件夹清理）；**用户可见设置只写 `config.json`**（`lua_ext.params`），
  别拿 data_dir 代替设置。
- 信任模型一句话：**装脚本 = 交出这个账号的权限**，与装任何来源的 exe 同级——没有沙箱
  （API.md §4 信任框）。

## 6. 调试与发布

标准闭环（判据由工具算，不靠眼睛）：

```
写脚本 → lua_check（warnings 必须为空）→ lua_trace（verdict 必须 pass）→ set_active（进面板）
```

- 有文件权限：直接写文件 + `curseen.exe --lua-check / --lua-trace / --lua-shot`（见
  [`接入指南.md`](接入指南.md) §1）；只有 MCP 工具：`write_script` → `lua_check` → `lua_trace` → `set_active`。
- `lua_shot` 把某一帧渲染成 PNG 直接返回，看构图 / 对比度用。
- `verdict=pass` 但 `judged=false`：这类脚本（`ctx:image` 的图片类）离屏判不了，
  **画面必须真机验收**。
- 发布到在线脚本库：按 `CurSeen-Scripts` 仓库的《开发指南.md》打包（pack.json + 双镜像直链），
  用户从「脚本」页的资源下载安装。

## 7. 排错速查

| 症状 | 先查 |
|---|---|
| 面板下拉里找不到脚本 | 放错挂载目录；或 `role` / `requires` 把它挡了（`--lua-check` 的返回里有 `ui_entry`） |
| 参数拖了没反应 | 参数没被脚本真的读（`lua_check` 报 warning）；或被面板裁剪（见 §3 末） |
| 改了 schema 默认值，画面没变化 | §3 的 `default` 兜底口径：脚本里 `or` 的值没跟着改 |
| 鼠标停住后特效冻在半空 | 悬浮标没续帧：动画未收敛时 `draw` 要返回正数毫秒 |
| 特效永不消失 | 点击效果没停笔；或静止后仍在续帧（`settles` / `stops` 判据会红） |
| `verdict=pass` 但画面不对 | 看 `judged`：`false` = 离屏判不了，真机看 |
| 出图报"图没出来" | `lua_shot` 的图在返回的图片块里；`out` 路径相对程序目录 |
| 改了配置没生效 | 先退出程序再改（运行中会被内存配置覆盖），首次启用要重启一次；脚本文件改完会热重载 |
