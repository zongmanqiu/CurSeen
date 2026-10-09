# CurSeen Lua 脚本 API 参考（完整版）

> 本文件是 **Lua 脚本可用 API 的权威参考**，写给写脚本的人。
> 与 `README.md`（作者向教程，含示例与排错表）互补：这里是**逐条、穷尽**的清单，
> 那里是「怎么上手」的引导。两者冲突以源码（`src/scripting/api_*.rs`、
> `src/scripting/engine.rs`）与本仓库文档为准。
>
> ★ 所有脚本都跑在一个**受限 Lua 5.4 环境**里：标准库只开了 `table` / `string` /
> `math` / `package`（外加始终加载的 `base`，即 `pairs` / `ipairs` / `type` /
> `tostring` / `pcall` / `error` 等基础语言特性）。**`io` / `os` / `debug` /
> `coroutine` / `utf8` 未加载**——脚本不能自行做文件 I/O 或加载磁盘模块，要用文件读
> `ctx.fs`、要联网用 `ext:http`、要算时间用 `wall_ms` / `elapsed`。

---

## 0. 两个 `ctx` 的约定（极易混淆）

脚本里会出现两个名字都叫 `ctx` 的东西，**语义完全不同，不要混用**：

| | 绘图模型 `draw(ctx, params)` 的 `ctx` | 运行模型 `run(ctx, params)` 的 `ctx` |
|---|---|---|
| 类型 | 全局 **userdata** `DrawListCtx`（不是 table） | 宿主传入的 **Lua table** |
| 怎么用 | `ctx:circle(...)` 冒号调用发绘图指令 | `ctx.role` / `ctx.fs` / `ctx.image` 点访问 |
| 字段 | **没有** `role` / `fs` / `ext` 字段 | **有** `role` / `fs` / `ext` 等 |
| 出现于 | 绘制特效（`role="effect"`）的 `draw` 回调 | `image` / `keysound` 角色 |

> 绘图 userdata 没有 `fs` / `role`；运行 table 没有绘图方法。误用在非对应回调里会静默
> 失效或报错，按上表区分即可。

---

## 1. 全局函数（Rust 注入到 Lua 全局表）

### 1.1 颜色 / 随机 / 数学助手

| Lua 名 | 签名 | 返回 | 说明 |
|---|---|---|---|
| `hex_to_rgb` | `hex_to_rgb(hex: string) -> (r, g, b, a)` | 4 个 `u8`（0–255） | 解析 `#RRGGBB` 或 `#RRGGBBAA`。**8 位顺序是 R,G,B,A**；6 位串 a 恒 255。格式错（含非法字符 / 多字节字符 / 长度不对）不 panic：长度不对整体兜底 `(255,0,0,255)`，字符非法逐通道兜底（RGB→0、A→255）。 |
| `rng` | `rng(min, max) -> number` | 浮点 | 区间随机浮点。**区间不可用（max≤min 或含 NaN）时退化返回 `min`**（`min` 本身为 NaN 时返回 0，防渲染帧崩溃）。 |
| `rng_int` | `rng_int(min, max) -> integer` | 整数 | 闭区间随机整数，**min>max 时退化返回 `min`**。 |
| `clamp` | `clamp(v, lo, hi) -> number` | 浮点 | 把 `v` 夹到 `[lo, hi]`（不修改原值）。 |
| `lerp` | `lerp(a, b, t) -> number` | 浮点 | 线性插值，`t` 被 clamp 到 `[0,1]`。 |

### 1.2 HSV 助手

| Lua 名 | 签名 | 返回 | 说明 |
|---|---|---|---|
| `hsv_to_rgb` | `hsv_to_rgb(h, s, v) -> (r, g, b)` | 3 个 `u8` | 色相 `h` 单位**度**（0–360），`s`、`v` 为 0–1，返回 0–255 的 RGB。 |

### 1.3 复合脚本专属：播放音效

> **仅 `role = "composite"`**（宿主 `host="panel"`）的实例 VM 注入了以下函数；
> 其它角色调用会报「找不到全局」。

| Lua 名 | 签名 | 返回 | 说明 |
|---|---|---|---|
| `play_sound` | `play_sound(file: string, volume?: integer) -> boolean` | 布尔 | `volume` 缺省 100，clamp 到 0–100。文件解析链与击键音效「音效文件」行同源（绝对路径 → 便携相对 → `assets/Sounds/` 裸名补扩展名）。返回是否真的发起了播放；找不到文件返回 `false`。 |

---

## 2. 全局环境变量（每帧由宿主注入，脚本只读）

> 这些变量在脚本加载时给默认值，之后**每帧被刷新**。**只有走绘图 / 特效宿主**
> （`trail` / `hover` / `clickfx` / `panel`）的脚本才会被刷新；走 `run` 模型的
> `image` / `keysound` 脚本不刷新这些全局，应改用 `ctx.role` 等。

| Lua 名 | 类型 | 语义 | 备注 |
|---|---|---|---|
| `screen_w` | number | 屏幕宽度（像素） | 默认 1920，每帧刷新 |
| `screen_h` | number | 屏幕高度（像素） | 默认 1080，每帧刷新 |
| `elapsed` | number | 逐帧累加时间（**秒**） | 覆盖层休眠 / 光标静止不出帧时不前进——**只适合做逐帧动画相位**，**不能**用它测「距某事件过了多久」 |
| `cursor_x` / `cursor_y` | number | 指针当前位置 | 点击效果里 = 按下点 |
| `mouse_x` / `mouse_y` | number | `cursor_*` 的别名 | 兼容老脚本，零改动 |
| `origin_x` / `origin_y` | number | 实例出生点 | 拖尾首个采样点 / 点击按下点 / 悬浮标当前位置 |
| `host` | string | 宿主标识 | `"trail"` / `"hover"` / `"clickfx"` / `"panel"`；`image`/`keysound` 不刷此全局，改用 `ctx.role` |
| `wall_ms` | number | **真实墙钟毫秒**（浮点） | 覆盖层休眠时也随真实时间前进。**静止检测 / 超时淡出必须用它，不要用 `elapsed`** |
| `_params` | table | 当前参数表的全局别名 | 兼容老脚本读全局 `_params`；所有回调都会把它设为与传给脚本的 `params` 相同的 table |

---

## 3. 绘图上下文 `ctx`（全局 userdata `DrawListCtx`）

通过 `ctx:方法(...)` 调用。颜色参数统一为 `(r, g, b, a)` 四个 `u8`（0–255）。
**若在非 `draw` 期间误调用（如 `init` 里），`TARGET` 未设置，静默丢弃、绝不 panic。**

| 方法 | 签名 | 说明 |
|---|---|---|
| `circle` | `(x, y, radius, r, g, b, a)` | 填充实心圆（SourceOver）。 |
| `clear_circle` | `(x, y, radius)` | 把圆内像素清成完全透明（Blend::Clear）；聚光灯开光圈用。颜色被忽略。 |
| `layer` | `(name: string)` | 后续指令归入层：`"base"`（垫底先画）/ `"top"`（顶层后画）/ `"normal"`（默认）。未知名按 normal 并告警。破坏性擦除（挖洞）必须垫底。 |
| `ring` | `(x, y, outer_r, width, r, g, b, a)` | 圆环。`outer_r` 外半径，`width` 从外径向内收缩（中心线半径 = outer_r − width/2）；`width ≥ outer_r` 退化为实心圆。 |
| `image` | `(x, y, scale, a)` | 贴出宿主预注入的悬浮标位图（`set_hover_image`），以 (x,y) 为**中心**，尺寸 = 原尺寸 × `scale`（缩放倍数，非边长），`a` 为整体透明度。`scale` 下限 0.01。无图时静默跳过。 |
| `rect` | `(x, y, w, h, r, g, b, a)` | 轴对齐填充矩形（实色快路）。 |
| `line` | `(x1, y1, x2, y2, width, r, g, b, a)` | 描边线段，圆头圆角（Round cap/join）。 |
| `diamond` | `(x, y, size, r, g, b, a)` | 菱形（四顶点 `size` 半对角线），加色混合（Blend::Plus）。 |
| `glow` | `(x, y, radius, r, g, b, a)` | 三层辉光（半径 ×2.5 / ×1.2 / ×1 叠加，alpha 0.12 / 0.30 / 1.0），加色混合。 |
| `plus_circle` | `(x, y, radius, r, g, b, a)` | 加色（Blend::Plus）实心圆；颜色已烘进 alpha，cmd alpha 恒 1.0。 |
| `radial` | `(x, y, radius, r, g, b, a)` | 径向渐变圆：中心 color → 边缘透明（SourceOver）。 |
| `diamond_rot` | `(x, y, s, angle, r, g, b, a)` | 旋转菱形（按 `angle` 弧度旋），加色混合。 |
| `polygon` | `(points, r, g, b, a)` | 普通混合（SourceOver）填充多边形。`points` 为 `{{x,y}, {x,y}, …}`（每个点 table 取 `[1]=x,[2]=y`）。点数 < 3 不绘制。 |
| `fill_polygon` | `(points, r, g, b, a)` | 加色版（Blend::Plus）填充多边形，参数同 `polygon`。 |
| `text` | `(x, y, s, size, r, g, b, a)` | 文字，锚点为排版盒**左上角**。 |
| `text_centered` | `(cx, cy, s, size, r, g, b, a)` | 文字，锚点为**几何中心**（点击点周围随机冒字用）。 |

---

## 4. 扩展 API：全局 userdata `ext`

每个 VM 都被设为全局 `ext`，**所有脚本类型都能用**；调用用冒号语法 `ext:xxx(...)`。

| 成员 | 形式 | 签名 / 返回 | 说明 |
|---|---|---|---|
| `get_image` | 方法 | `ext:get_image() -> ExtImage | nil` | 当前贴图（仅贴图动作上下文有值；脚本页 / 无贴图时返回 `nil`）。 |
| `result` | 函数 | `ext:result(text: string)` | 弹出一个可复制文本框（非阻塞，自带消息泵）。 |
| `log` | 函数 | `ext:log(msg: string)` | 走 Rust 日志（`[ext] msg`），便于排障。 |
| `http` | 函数 | `ext:http(method, url, headers: table, body?: string) -> { status: int, body: string }` | **阻塞** HTTP（60s 超时）。`headers` 为 `key=value` 表；`method` 支持 GET / DELETE / POST / PUT（其它方法返回 `{status=0, body="不支持的 HTTP 方法…"}`）。错误时 `status=0` 且 `body` 为错误信息字符串（不 panic）。 |
| `input` | 方法 | `ext:input(title: string, default?: string) -> string \| nil` | 弹一个可编辑输入框（阻塞，自带消息泵）。确定 = 返回输入文本；取消 / `Esc` / 关窗 = 返回 `nil`。 |
| `json_decode` | 函数 | `ext:json_decode(text: string) -> table \| nil` | JSON 文本 → Lua 表（数组 → 1 起始序列表）。解析失败返回 `nil`（不抛错）。 |
| `json_encode` | 函数 | `ext:json_encode(value) -> string` | Lua 表 / 标量 → JSON 文本。`1..n` 连续整数键的表 = 数组，其余 = 对象；不支持的值（函数等）→ `null`。 |

### 4.1 `ExtImage` userdata

由 `ext:get_image()` 与 `run` 模型的 `ctx.image` 返回。只读快照克隆，脚本改动不回写贴图。

| 成员 | 形式 | 签名 / 返回 | 说明 |
|---|---|---|---|
| `w` / `h` | 只读字段 | `img.w -> int` / `img.h -> int` | 位图宽 / 高。 |
| `png_base64` | 方法 | `img:png_base64() -> string` | 编码为 base64 PNG 文本（在线接口用）。 |
| `save` | 方法 | `img:save(path: string)` | 存成 PNG 文件（父目录会自动创建）。 |
| `pixel` | 方法 | `img:pixel(x, y) -> r, g, b, a` | 逐点取 RGBA（4 个 int）。越界（负 / 超宽高）返回**全 nil**，脚本自行判断。 |
| `bytes` | 方法 | `img:bytes() -> string` | 原始 RGBA 缓冲（二进制安全字符串，长度 = w×h×4，行优先）。批量采样比逐点 `pixel` 快一个量级。 |

---

## 5. 文件系统：`ctx.fs`（只读）

仅 **`run(ctx, params)` 模型**的脚本（`image` / `keysound`）通过 `ctx.fs` 拿到。
根 = 脚本所在目录。逃逸校验：绝对路径、`..` 父级、`C:\` / `/` 前缀一律返回 `nil` /
`false`（防手滑越界，**非安全沙箱**）。

| 方法 | 签名 | 返回 | 说明 |
|---|---|---|---|
| `read` | `fs:read(rel: string) -> string | nil` | 文本读取；不存在 / 越界 / 读失败返回 `nil`。 |
| `read_bytes` | `fs:read_bytes(rel: string) -> string | nil` | 二进制读取（编码安全字符串）。 |
| `exists` | `fs:exists(rel: string) -> bool` | 存在性（不区分文件 / 目录）。 |
| `list` | `fs:list(rel: string) -> { names... } | nil` | 目录条目（文件名 / 目录名，不含 `.` / `..`）；读取失败返回 `nil`。 |

---

## 6. `run(ctx, params)` 模型的 `ctx` table 字段

仅 `image` 与 `keysound` 角色走 `run`。`ctx` 由宿主构造并固定字段：

| 字段 | 含义 | 出现于 |
|---|---|---|
| `ctx.role` | 角色字符串（`"image"` / `"keysound"` 等） | 所有 run 脚本 |
| `ctx.fs` | 只读文件访问 userdata（见 §5） | 所有 run 脚本 |
| `ctx.ext` | `ExtApi` 引用（`ext` 全局的副本） | 所有 run 脚本 |
| `ctx.image` | `ExtImage` userdata（仅由贴图右键动作触发时有值） | `image` 角色 |
| `ctx.hwnd` | 结果框 / 弹窗 owner 窗口句柄（整数） | `image` 角色 |
| `ctx.sounds` | 音效候选名数组（1 起索引的 table） | `keysound` 角色 |
| `ctx.mods` | 修饰键表 `{ ctrl, alt, shift, win }`（bool） | `keysound` 角色 |
| `ctx.key` | 键名字符串（与击键回显同源，如 `"A"` / `"F5"` / `"Space"`） | `keysound` 角色 |
| `ctx.repeated` | 是否长按自动重复（bool） | `keysound` 角色 |
| `ctx.dt` | 距上次按键时长（整数毫秒，**当前恒 0**，占位） | `keysound` 角色 |

> `run` 模型脚本**不**刷新 §2 的全局 `host` / `cursor_*` / `elapsed` 等，需要环境信息时
> 改用 `ctx.role` 或自行维护。

---

## 7. 脚本可声明的顶层全局（由 Rust 读取，非脚本调用）

| 全局名 | 类型 | 读取方 | 说明 |
|---|---|---|---|
| `effect_name` | string | 特效三目录扫描取显示名；缺失回退文件名 | 特效显示名。 |
| `name` | string | 统一框架脚本取显示名，缺省文件名 | 显示名。 |
| `description` | string | 统一框架 | 一句话说明（可选）。 |
| `role` | string | 注册表 | `image` / `effect` / `keysound` / `composite`；缺省推演见 §8。 |
| `menu` | string | 统一框架 | 贴图右键「图片操作」子菜单文本；`nil` 表示只出现在脚本页。 |
| `hide_cursor` | bool | 引擎读取全局 | 脚本声明隐藏系统指针时设 `true`；不存在 / 非布尔 → `false`。必须在 `load_script` 之后读取（顶层赋值在执行期写入 globals）。 |

---

## 8. 脚本必须 / 可以定义的回调（函数）

这些**不是** Rust 预定义函数，而是宿主在合适时机反向调用脚本里定义的全局函数。
所有 `params` 参数都是 `schema()` 声明的参数表（缺键时用 `params.x or 默认` 兜底；也可读全局 `_params`）。

| 回调 | 签名 | 被谁调用 | 说明 / 返回值契约 |
|---|---|---|---|
| `schema` | `schema() -> table` | 注册表扫描期 + 运行时 | **必需**（否则无参数 UI）。返回参数声明数组。 |
| `defaults` | `defaults() -> table` | 引擎 | **遗留兼容**：老脚本不写 `schema` 时回退用。新脚本请用 `schema`。 |
| `init` | `init()` | 各宿主 | 可选。启用 / 热重载后调一次，清零状态。缺失不报错。 |
| `on_move` | `on_move(x, y, t, params)` | 拖尾 / 悬浮标 / 点击效果 / 复合 | 鼠标采样。`t` = 共享时钟（秒）。 |
| `step` | `step(dt, t, params)` | 同上 | 每帧推进。**`dt` 单位 = 秒**；老化 / 淡出必须放这里（鼠标不动也要推进）。 |
| `draw` | `draw(ctx, params) -> number?` | 同上 | 每帧绘制。返回值语义 = 续帧请求：返回 `n>0` = "n 毫秒后再来一帧"；返回 0 / 负数 / 其它类型 / `nil` = 不请求额外帧。**只对悬浮标（`HoverDot/`）真正成立**；拖尾 / 点击不看返回值（靠"是否还在画"判活）。 |
| `on_press` | `on_press(x, y, t, params)` | 全部宿主 | 指针按下触发一次。**兼容老名 `on_click`**（先找 `on_press`，没有再回退 `on_click`，都没有则静默跳过）。 |
| `on_mouse_down` | `on_mouse_down(button, x, y, params)` | 复合脚本专属 | 鼠标按下，`button` ∈ `"left"/"right"/"middle"/"x1"/"x2"`（参数顺序刻意与 `on_press(x,y,t)` 不同名，防抄错）。取时间用全局 `elapsed`。 |
| `on_key` | `on_key(key, mods, repeated, params)` | 复合脚本专属 | 键盘按下。`key` = 键名；`mods` = `{ctrl,alt,shift,win}`；`repeated` = 是否长按重复。返回值当前未被宿主解释，发声请显式调 `play_sound()`。 |
| `run` | `run(ctx, params) -> value` | `image` / `keysound` 角色入口 | 无全局 `run` → 返回 `nil`。 |

> 回调容错：除 `schema` / `init` 外，缺失的回调**静默跳过**（不报错、不 panic）。纯绘制脚本
> 不写 `on_press` / `step` 完全正常。

---

## 9. `schema()` 参数表结构

`registry.rs::parse_schema_table` 解析 `schema()` 返回的数组，每项为带字段的 table：

| 字段（Lua 键） | 类型 | 含义 |
|---|---|---|
| `key` | string | **配置键**，脚本里 `params.<key>` 取用。缺省 / 空则跳过该项。 |
| `label` | string | UI 显示名；缺省 = `key`。 |
| `type` | string | 参数类型（见下表）。未知 / 缺省 → `"float"`。 |
| `min` / `max` | number | 滑块 / 数值范围（bool / choice / sound / folder / str / file / hotkey 忽略）。缺省 0 / 1。 |
| `step` | number | 滑块步进（仅 UI）。缺省 0.01。 |
| `default` | number / string / bool | 默认值；缺失时按类型给兜底。 |
| `choices` | table(array) | `choice` 类型的候选项字符串（也是存进配置、喂给脚本的稳定值，勿改）。 |
| `labels` | table(array) | `choice` 各选项界面显示名（可选，与 `choices` 一一对应；长度不符则整份丢弃）。 |
| `group` | string | 分组名（UI 分组标题，可选）。 |
| `alpha` | bool | **仅 `color` 用**。为 `true` 时取色器带透明度条、落盘走 8 位 `#RRGGBBAA`、脚本 `hex_to_rgb` 第 4 返回值拿到 0–255 alpha；未开则恒 6 位（`alpha` 恒 255）。 |

**`type` 取值 → 面板控件**：`float`（滑条+输入框）/ `int`（整数滑条）/ `bool`（开关）/
`color`（取色器，`alpha=true` 出透明度条）/ `choice`（下拉）/ `sound`（音效下拉）/
`folder`（文件夹）/ `image`（图片接口，UI 占位）/ `str`（文本框）/ `file`（文件浏览）/
`hotkey`（快捷键捕获框：点击后按下的组合键写回框内，值 = 规范化按键串如 `Ctrl+Alt+H`，
空串 = 停用）。★ `hotkey` 只有 `scripts/` 独立脚本会被当成**全局热键**触发运行；
特效 / 贴图脚本声明它只是存一个键位串。

> ★★ **颜色串只有两种写法：`#RRGGBB` 与 `#RRGGBBAA`（alpha 在末尾）**。项目里**
> 不存在** `#AARRGGBB`。`hex_to_rgb` 的第 4 返回值对应 alpha（0–255）。

---

## 10. 四类脚本角色（`role`）与宿主差异

挂载点 = **目录**（不是脚本里的声明）：

| 目录 / 角色 | `role`（缺省推演） | 宿主 `host` | 模型 | 可用回调 |
|---|---|---|---|---|
| `assets/lua/MouseTrail/` | `effect` | `"trail"` | 绘图回调 | `init`, `on_move`, `step`, `draw` |
| `assets/lua/HoverDot/` | `effect` | `"hover"` | 绘图回调 | `init`, `draw`（可用 `ctx:image` 贴预载位图） |
| `assets/lua/ClickFx/` | `effect` | `"clickfx"` | 绘图回调 | `on_press`/`on_click`, `step`, `draw` |
| `assets/lua/scripts/` | 缺 `role` 且无 `menu` → **`composite`** | `"panel"` | 绘图回调 + `play_sound` | `init`, `on_move`, `on_mouse_down`, `on_key`, `step`, `draw` |
| 贴图动作脚本 | 有 `menu` 无 `role` → `image` | —（走 `run`） | `run(ctx, params)` | `run` |
| 击键音效脚本 | `keysound` | —（走 `run`） | `run(ctx, params)` | `run` |
| 其它目录散件 | 无 `role` 且无 `menu` 又不在上述目录 → **告警并跳过** | — | — | — |

---

## 11. 关键返回值契约速查

| 场景 | 返回值 | 含义 |
|---|---|---|
| `draw(ctx, params)` | `nil` / 0 / 负数 / 其它类型 | 不请求额外帧（静止时零功耗）。 |
| `draw(ctx, params)` | `n > 0` | 请求 n 毫秒后再来一帧（悬浮标淡出靠它跑完）。 |
| `run`（image） | 非空 `string` | 自动弹结果框显示该文本。 |
| `run`（image） | `nil` / 空串 | 脚本已自行用 `ext:result` 弹框。 |
| `run`（keysound） | `false` | **强制静音**（连宿主默认音也不响）。 |
| `run`（keysound） | `string`（非空，去空白） | 当作音效路径 / 名，经 `stored_file_path` 解析播放。 |
| `run`（keysound） | `integer` / `number` | 当作 `ctx.sounds` 数组的 1 起索引，选中该候选音；越界 / ≤0 落回默认。 |
| `run`（keysound） | `nil` / `true` / 其它 | 落回宿主默认音（出厂 `click3`）。 |

---

## 12. 实现位置索引（按文件）

| 文件 | 负责 |
|---|---|
| `scripting/engine.rs` | VM 生命周期 + 所有 `call_*` 反向调用 + 环境注入（`update_env` / `set_wall_ms`）+ `get_effect_name` / `get_hide_cursor` + 全局 `ext` 注册 |
| `scripting/api_draw.rs` | 绘图 userdata `DrawListCtx`（§3）+ 全局 `ctx` / `hsv_to_rgb` 注册 |
| `scripting/api_effect.rs` | 颜色 / 随机 / 数学全局函数（§1.1）+ 环境变量初值（§2） |
| `scripting/api_ext.rs` | `ext` userdata `ExtApi`（§4）+ `ExtImage`（§4.1） |
| `scripting/fs_api.rs` | `ctx.fs` 只读文件访问（§5）+ 逃逸校验 |
| `scripting/registry.rs` | 特效三目录 Manifest 扫描 + `ParamType` / `ParamDef` / `EffectSchema` |
| `scripting/registry_ext.rs` | 统一框架（递归扫 `assets/lua/`）Manifest + `role` 推演 |
| `scripting/ext_runner.rs` | 贴图 / 脚本页扩展脚本执行器（`run(ctx,params)` + `ctx.image` / `ctx.hwnd` 注入） |
| `scripting/runtime.rs` | 宿主级 VM 缓存 `EngineCache`（热更新 + GC） |
| `modules/trail/companion.rs` | 复合脚本宿主：注入 `play_sound`、`host="panel"`、调用各回调 |
| `modules/key_sound.rs` | `keysound` 宿主：构造 `ctx.sounds/mods/key/repeated/dt` 并 `run` |
| `modules/trail/fx_core.rs` | `FxCore` 驱动（拖尾 / 悬浮标 / 点击效果 / 复合共用的引擎） |
