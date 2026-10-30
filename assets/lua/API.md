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
| `screen_x` / `screen_y` | number | 虚拟桌面原点（物理像素，可为负） | 整屏暗罩 / 星空 / 镜像中心要用它：副屏在主屏左侧或上方时 `0,0` 不是屏幕左上角 |
| `elapsed` | number | 逐帧累加时间（**秒**） | 覆盖层休眠 / 光标静止不出帧时不前进——**只适合做逐帧动画相位**，**不能**用它测「距某事件过了多久」 |
| `cursor_x` / `cursor_y` | number | 指针当前位置 | 点击效果里 = 按下点 |
| `mouse_x` / `mouse_y` | number | `cursor_*` 的别名 | 兼容老脚本，零改动 |
| `cursor_vx` / `cursor_vy` | number | 指针速度（**60fps 每帧像素**） | 与 CursorFX 的 `state.vx/vy` 同量纲 |
| `cursor_speed` / `cursor_heading` | number | 速度标量 / 运动方向（弧度） | 方向 = `atan2(vy, vx)` |
| `mouse_vx` / `mouse_vy` / `mouse_speed` / `mouse_heading` | number | `cursor_*` 的别名 | 移植 CursorFX 脚本时零改名 |
| `mouse_down` | bool | 当前是否按住鼠标 | 按住态视觉（加速/后坐/连发） |
| `mouse_button` | string | 最近按下的键：`left/right/middle/x1/x2` | 抬起后保留最后键名 |
| `cursor_hover` | bool | 系统是否正在给出**交互光标提示** | I 形 / 手形 / 缩放 / 禁止 / 帮助 / 十字。桌面按钮用箭头光标，所以它**不代表**"指针下有按钮"；自定义光标也不算。 |
| `origin_x` / `origin_y` | number | 实例出生点 | 拖尾首个采样点 / 点击按下点 / 悬浮标当前位置 |
| `host` | string | 宿主标识 | `"trail"` / `"hover"` / `"clickfx"` / `"panel"`；`image`/`keysound` 不刷此全局，改用 `ctx.role` |
| `wall_ms` | number | **系统运行毫秒**（浮点，非 Unix 时间） | 覆盖层休眠时也随真实时间前进；**静止检测 / 超时淡出用它** |
| `local_ms` | number | **本地 Unix 毫秒**（含时区偏移） | 时钟类脚本直接 `/1000` 取本地时分秒；`offset=0` 即系统本地时间 |
| `unix_ms` / `tz_offset_min` | number | UTC Unix 毫秒 / 本地时区分钟偏移 | 需要自己算时区时使用 |
| `_params` | table | 当前参数表的全局别名 | 兼容老脚本读全局 `_params`；所有回调都会把它设为与传给脚本的 `params` 相同的 table |

---

## 3. 绘图上下文 `ctx`（全局 userdata `DrawListCtx`）

通过 `ctx:方法(...)` 调用。颜色参数统一为 `(r, g, b, a)` 四个 `u8`（0–255）。
**若在非 `draw` 期间误调用（如 `init` 里），`TARGET` 未设置，静默丢弃、绝不 panic。**

| 方法 | 签名 | 说明 |
|---|---|---|
| `circle` | `(x, y, radius, r, g, b, a)` | 填充实心圆（SourceOver）。 |
| `ellipse` | `(x, y, rx, ry, r, g, b, a)` | 填充椭圆（SourceOver）。 |
| `ellipse_ring` | `(x, y, rx, ry, width, r, g, b, a)` | 椭圆描边（空心，圆头圆角）。 |
| `round_rect` | `(x, y, w, h, radius, r, g, b, a)` | 填充圆角矩形；`radius` 自动夹到短边一半。 |
| `round_rect_ring` | `(x, y, w, h, radius, width, r, g, b, a)` | 圆角矩形描边。 |
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
| `radial_stops` | `(x, y, radius, stops)` | 多段径向渐变圆。`stops = {{offset=0,r=..,g=..,b=..,a=..}, …}`，至少 2 段；offset 夹到 0–1。 |
| `diamond_rot` | `(x, y, s, angle, r, g, b, a)` | 旋转菱形（按 `angle` 弧度旋），加色混合。 |
| `polygon` | `(points, r, g, b, a)` | 普通混合（SourceOver）填充多边形。`points` 为 `{{x,y}, {x,y}, …}`（每个点 table 取 `[1]=x,[2]=y`）。点数 < 3 不绘制。 |
| `fill_polygon` | `(points, r, g, b, a)` | 加色版（Blend::Plus）填充多边形，参数同 `polygon`。 |
| `text` | `(x, y, s, size, r, g, b, a)` | 文字，锚点为排版盒**左上角**。 |
| `text_centered` | `(cx, cy, s, size, r, g, b, a)` | 文字，锚点为**几何中心**（点击点周围随机冒字用）。 |
| `backdrop_circle` | `(x, y, radius, mode, zoom, r, g, b, alpha) -> bool` | **真·桌面采样**：抓取指针周围一小块桌面，按圆形蒙版画回本帧。`mode="magnify"` = 放大镜（源区半径 = `radius/zoom`，zoom 1–4）；`mode="difference"` = 逐通道 `\|源−(r,g,b)\|`，复刻 CSS `difference`。`alpha` 与其它颜色参数同口径（0–255）。返回 `false` = 抓屏或自身排除不可用（脚本应退回静态外观）。★ 只要它在跑，CurSeen 覆盖层对系统截图/录屏不可见——这是排除自身、避免镜片无限套娃的代价。 |

---

## 4. 扩展 API：全局 userdata `ext`

每个 VM 都被设为全局 `ext`，**所有脚本类型都能用**；调用用冒号语法 `ext:xxx(...)`。

| 成员 | 形式 | 签名 / 返回 | 说明 |
|---|---|---|---|
| `get_image` | 方法 | `ext:get_image() -> ExtImage \| nil` | 当前贴图（仅贴图动作上下文有值；脚本页 / 无贴图时返回 `nil`）。 |
| `result` | 函数 | `ext:result(text: string)` | 开一扇**宿主文本窗**（标题带 + 一个多行文本框）：会换行、鼠标直接圈选、Ctrl+C 可复制，还能**轻微改两下**再走；底部「复制」交的是**当前**内容（改过就复制改后的）。窗高按行数实算。阻塞在**脚本自己的线程**上直到用户关窗，UI 线程零等待。 |
| `log` | 函数 | `ext:log(msg: string)` | 走 Rust 日志（`[ext] msg`），便于排障。 |
| `http` | 函数 | `ext:http(method, url, headers: table, body?: string) -> { status: int, body: string }` | **阻塞** HTTP（60s 超时）。★ 只能在 `run` 模型里调：`draw` 逐帧跑在 UI 线程，调用会被**拒绝**（返回 `status=0` 与原因），不会把界面冻住。`headers` 为 `key=value` 表；`method` 支持 GET / DELETE / POST / PUT（其它方法返回 `{status=0, body="不支持的 HTTP 方法…"}`）。错误时 `status=0` 且 `body` 为错误信息字符串（不 panic）。 |
| `download` | 方法 | `ext:download(url: string, path: string, headers?: table) -> int \| nil, err` | 把 URL 的响应体**流式**写到 `path`（二进制安全，返回字节数）。★ 先写 `path.partial`、成功再原子改名，失败清半成品；上限 512 MB；服务端给了 `Content-Length` 会校验。★ **阻塞**，只能从 `run` 模型里调（`draw` 跑在 UI 线程，会冻全屏）。★ 地址不限——和 `ext:http` 一样，下什么、从哪下是脚本作者的责任。 |
| `unzip` | 方法 | `ext:unzip(zip_path: string, dest_dir: string) -> int \| nil, err` | 把 ZIP 解到 `dest_dir`（不存在就建），返回**文件**数（目录不算）。只认 stored / deflate；**路径逃逸一律拒**（`..`、绝对路径、盘符、符号链接），CRC 不符 / 加密条目 / 超限（单条目 256 MB、整包 2 GiB）也拒，且不留半成品。暂不支持 ZIP64。 |
| `input` | 方法 | `ext:input(title: string, default?: string) -> string \| nil` | 开同一扇宿主文本窗的**输入**档（多行，Enter 换行；进窗即全选，提交只走「确定」按钮）。确定 = 返回输入文本；取消 / `Esc` / 关窗 = 返回 `nil`。★ 只有 `run` 模型（`image` / `keysound`，跑在工作线程）能等到回答：`draw` 模型逐帧跑在 UI 线程，在那里阻塞会自锁，所以那种调用只开窗、立即返回 `nil`。 |
| `proc` | 方法 | `ext:proc(opts: table) -> table \| nil, err` | **外置工具通道**（m10）：起一个进程、等它跑完、把输出交回脚本。`opts = { exe, args?, cwd?, timeout_ms?, ansi? }`，返回 `{ code, stdout, stderr, timed_out, truncated, elapsed_ms }`（`code` 在被超时杀掉时是 `nil`；`elapsed_ms` 是**整次调用**的墙钟，含起进程与加载 dll——那才是用户感到的延迟）。★ `exe` 只准写**脚本自己目录（含子目录）里的相对路径 + `.exe`**：绝对路径、`..`、`.dll` / `.bat` 一律拒绝并给中文原因。`args` 是**数组**，逐个交给子进程，不拼命令行（路径里有空格与引号都不用脚本操心）。`ansi = true` 按系统码页（中文系统 = GBK）解输出，默认按 UTF-8——猜错的表现就是"锟斤拷 / 鏂囧瓧"，所以这条只能由认识那个工具的脚本来声明。`timeout_ms` 默认 30 s、上限 120 s（超时宿主一定杀掉并收尸）。工作目录默认 = exe 所在目录（工具按相对路径找自己的模型 / dll）。阻塞在脚本自己的线程上，UI 零等待。★ 只能在 `run` 模型里调（`draw` 在 UI 线程上调用会被拒绝，理由同 `ext:http`）。 |
| `script_dir` | 方法 | `ext:script_dir() -> string` | 脚本自己所在目录的**绝对**路径。要给外置工具拼绝对路径时用它（多数工具只认绝对路径）。 |
| `tmpdir` | 方法 | `ext:tmpdir() -> string` | 宿主专属的临时目录（绝对路径，取用时已建好）。`ext:proc` 路线的标准用法：`img:save(ext:tmpdir() .. "\\in.png")` → 把绝对路径交给工具。 |
| `data_dir` | 方法 | `ext:data_dir() -> string` | 本脚本自己的**持久数据目录**：`<程序目录>/data/lua/<脚本 id>/`（绝对路径，按需创建）。缓存/历史/上次选择放这里；不在 `assets/` 下，写它不会触发目录监听，也不会被 `temp/` 清理。**用户可见设置仍写 `config.json` 的 `lua_ext.params[id]`，不要拿这里代替设置。** |
| `open_file` | 方法 | `ext:open_file(kind?: string, start_dir?: string) -> string \| nil \| nil, err` | 原生「打开」对话框，**由用户来选**（所以这里的路径可以是任意绝对位置，与 `ext:proc` 的 `exe` 只准脚本目录相反——选什么是人做的，起始目录只是提示）。`kind` = `any`(默认) / `image` / `audio` / `gif` / `mp4`，**只影响过滤器**；不认识的写法返回 `nil, 原因` 并列出可用档（刻意**不降级成 any**：拼错一个字母却拿到全文件对话框，症状是"它有时候不给我显示图片"）。三档要分清：选到 = 绝对路径；**取消 = `nil`**（"没选"不是错误，脚本自己收尾）；词表不对 / 在 `draw` 脚本里调用 = `nil, 原因`。阻塞在脚本自己的线程上，对话框模态于触发它的那张贴图。 |
| `save_file` | 方法 | `ext:save_file(name?: string, ext?: string, start_dir?: string) -> string \| nil` | 原生「另存为」。`ext` 会被收成**前导一段**字母数字（转小写、≤8 字，畸形或缺省 = `txt`），所以 `"p.n"` 是 `p` 而不是 `pn`。返回的路径可能指向已存在的文件——覆盖确认由系统在对话框里问，脚本不必再自查存在性。取消 = `nil`。 |
| `open_temp` | 方法 | `ext:open_temp(bytes: string, ext: string) -> string \| nil, err` | 把字节写成 `ext:tmpdir()` 里的临时文件，并用**系统默认程序**打开；成功返回路径。`ext` 只允许光栅图片（`png/jpg/jpeg/gif/bmp/webp/tif/tiff/ico`），其余一律拒——二维码内容不可信，不能借 `.exe`/`.lnk`/`.url` 变成一次执行。写盘回读长度；打开失败时文件仍在，原因里带路径。 |
| `open_url` | 方法 | `ext:open_url(url: string) -> true \| nil, err` | 用系统默认浏览器打开网址。**只允许 `http` / `https`**；`file:`、`javascript:`、自定义协议与含空白/控制字符的串一律拒。由调用方决定是否把它做成开关（默认不要自动打开）。 |
| `read_file` | 方法 | `ext:read_file(path: string) -> string \| nil, err` | 读**任意路径**的字节（m14「回程」）。与 `ctx.fs:read` 的分工就在"任意路径"上：那个限脚本目录（防手滑的纪律），这个不限——外部工具的产物落在哪儿都取回来。返回 Lua 二进制安全字符串，**字节原样、不猜编码**。超过 64 MiB 一律**拒**而不是截断（被剪掉的字节当图片/JSON 用，症状是"这文件坏了"）。取消/失败分得清：文件不存在、是目录、超限 ⇒ `nil, 原因`。 |
| `write_file` | 方法 | `ext:write_file(path: string, data: string) -> true \| nil, err` | 写文件（任意路径，父目录自动建）。★ 写完**回读核长度**：`fs::write` 报 Ok 不等于盘上就是这些字节（占位云盘、同步杀软是这条路上唯一会骗人的一环），对不上就报原因——同 `ext:clip_set` 一条纪律，不报"写好了"。 |
| `load_image` | 方法 | `ext:load_image(path: string) -> ExtImage \| nil, err` | 把文件读成图片对象。有了它，"外部工具改了张图"才真接得回脚本：`ext:proc` → `ext:load_image` → 像素 / `png_base64` / 再喂下一个工具。★ **按原尺寸解**（m21）：走 WIC，格式支持面以**这台机器上 WIC 装了哪些解码器**为准（PNG / JPEG / BMP / GIF / TIFF 通常都有，WebP 要看系统；"本机能不能解"以实测为准，别假设"常见格式都行"）。三条拒绝面各给一句人话：文件不存在 / 0 字节 · 超过 128 MB（与剪贴板缩略图同一个上限）· 整张超过 64 百万像素（BGRA 是 4 字节/像素，再大就该先缩）。格式不认识 / 缺解码器 / 文件坏掉，WIC 只报同一个失败，所以原因里如实写"三种之一"。读回来的 **α 原样保留**（不是截图那条"α 一律刷成 255"的语义，见 `core::backdrop::shot_to_bitmap_alpha`）。 |
| `set_image` | 方法 | `ext:set_image(源) -> true \| nil, err` | 把**眼前这张贴图**换成另一张图（源 = `ExtImage`，或一个图片文件路径——与 `ext:load_image` 同一条读图路）。★ 语义与右键「旋转 / 镜像」**同一条缝**，因此行为一致：只换**视图**——不写盘，也不改「恢复原图」与贴图环用的那份**出厂原图**；新图宽高不同会带着窗口一起重算（图 × 当前缩放 + 2px 描边，左上角不动）。改坏了有退路：像素一旦与出厂那份不同，贴图右键就出现**「恢复原图」**，换回的正是钉进来那一刻那张。脚本侧不需要自己留原图。没有贴图上下文（脚本页里直接跑、热键触发时没选中窗）⇒ `nil, 原因`，**不猜是哪一张**。逐帧脚本里排到下一次派发才生效，第二返回值会这么说明。 |
| `clip_get` | 方法 | `ext:clip_get() -> string \| nil` | 读剪贴板里的文本（识别结果去别处粘贴前可以先看一眼）。没有文本 = `nil`。 |
| `clip_image` | 方法 | `ext:clip_image() -> ExtImage \| nil` | 剪贴板里的**位图**（CF_DIB，32/24bpp，自下而上/自上而下都认）。让"截图 → 分析"不必先钉成贴图：截图工具（Win+Shift+S / QQ 截图）放进剪贴板之后，直接 `ext:clip_image()` → 取色板 / 对比度体检 / `img:crop` → `img:save`。★ 没有图像 = `nil`（与 `clip_get` 没有文本同一条纪律；读失败也归到 `nil`——底层不区分，这里不假装分得清）。★ α 一律按**抓屏语义**刷成 255：DIB 的第四字节不可信，别指望它带透明（带透明的图请用 `ext:load_image`）。 |
| `clip_set` | 方法 | `ext:clip_set(text: string) -> true \| 抛错` | 把文本写进剪贴板。★ **写完立刻读回来核**：底层写入不返回结果（失败只落日志），不一致就抛错——不能替用户宣布"复制好了"。★ 这会覆盖用户原有的剪贴板内容（本机共享资源），所以面向人的脚本要把这一步做成**可关的开关**，并写清复制成功与否。 |
| `json_decode` | 函数 | `ext:json_decode(text: string) -> table \| nil` | JSON 文本 → Lua 表（数组 → 1 起始序列表）。解析失败返回 `nil`（不抛错）。 |
| `json_encode` | 函数 | `ext:json_encode(value) -> string` | Lua 表 / 标量 → JSON 文本。`1..n` 连续整数键的表 = 数组，其余 = 对象；不支持的值（函数等）→ `null`。 |

### 4.1 `ExtImage` userdata

由 `ext:get_image()` 与 `run` 模型的 `ctx.image` 返回。只读快照克隆，脚本改动不回写贴图。

| 成员 | 形式 | 签名 / 返回 | 说明 |
|---|---|---|---|
| `w` / `h` | 只读字段 | `img.w -> int` / `img.h -> int` | 位图宽 / 高。 |
| `png_base64` | 方法 | `img:png_base64() -> string` | 编码为 base64 PNG 文本（在线接口用）。 |
| `save` | 方法 | `img:save(path: string, quality?: int)` | 按**扩展名**写文件（m25）：`.png` 无损（给它传 `quality` 会报错），`.jpg` / `.jpeg` 有损（`quality` 1–100，默认 90，**透明像素合成到白底**——JPEG 没有 α 通道）。别的扩展名一律拒绝并点名这两种；配合 `ext:save_file(名字, "jpg")` 先让用户选落点。父目录自动创建；失败**抛错**，原因里点名阶段。 |
| `pixel` | 方法 | `img:pixel(x, y) -> r, g, b, a` | 逐点取 RGBA（4 个 int）。越界（负 / 超宽高）返回**全 nil**，脚本自行判断。 |
| `bytes` | 方法 | `img:bytes() -> string` | 原始 RGBA 缓冲（二进制安全字符串，长度 = w×h×4，行优先）。批量采样比逐点 `pixel` 快一个量级。 |
| `crop` | 方法 | `img:crop(x, y, w, h) -> ExtImage \| nil, err` | 裁出子矩形（**像素坐标，相对本图左上角**）。★ 越界**不钳制**：一律 `nil, 原因` 并点名是哪个角出的界——悄悄裁一半的症状是"它有时候少一块"，比一句"我算错了"难查得多。 |
| `rotate_cw90` | 方法 | `img:rotate_cw90() -> ExtImage` | 顺时针 90°，宽高互换（与贴图右键「顺时针 90°」同一个 core 函数）。 |
| `flip_h` / `flip_v` | 方法 | `img:flip_h() -> ExtImage` | 水平 / 垂直镜像。 |
★ **这组动作返回的是新图**：手里这张（快照）不动；要让屏幕上那张变，必须显式 `ext:set_image(结果)`。**没有任意缩放**：宿主只做上面这几件，缩放 / 换格式那一类走外部工具（`ext:proc`）——不为本体加采样器或解码依赖。

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

> ★ **信任模型只有一句：装脚本 = 交出你这个账号的权限**，和装任何来源的 exe 同级。这里**没有
> 沙箱**。`ctx.fs` 那套"限脚本目录、拒绝绝对路径与 `..`"是一条**防手滑的纪律**（写给整理配置的
> 脚本作者自己），不是安全边界；`ext:read_file` / `write_file` / `load_image` / `img:save` / `download` / `unzip` 与
> `ext:open_file` / `save_file` 都在边界外。`ext:proc` 那条"**只准起脚本自己目录里的 `.exe`**"
> 同样只是**路径收口**（见 §4）：它拦手滑与误装，拦不住恶意脚本——`ext:write_file` 就能把 exe
> 写进那个目录再被起起来。收口的真实价值是**可审计**：想知道脚本能跑什么，只看那一个目录。
> `ext:unzip` 会拒 `..` / 绝对路径 / 符号链接条目——那是**防手滑**（别把文件解到目标目录之外），同样不是沙箱。

## 7. 脚本可声明的顶层全局（由 Rust 读取，非脚本调用）

| 全局名 | 类型 | 读取方 | 说明 |
|---|---|---|---|
| `effect_name` | string | 特效三目录扫描取显示名；缺失回退文件名 | 特效显示名；`name` 优先，二者互为别名。 |
| `name` | string | 统一框架脚本取显示名，缺省文件名 | 显示名；复制到任何目录都先读它。 |
| `description` | string | 统一框架 | 一句话说明（可选）。 |
| `kind` | string | 包卡下拉 | `cursor` / `trail` / `click` / `hover` / `other`；官方目录以目录为准，包卡用它标类别。 |
| `category` | string | 脚本页卡片排序 | 可选分组：`mouse` / `picture` / `text` / `ai`。有值的卡片先按固定组顺序排；**没有声明的老脚本排在最后**、组内仍按原 id。只影响排序，不改显示名，也不改 id。 |
| `role` | string | 注册表 | `image` / `effect` / `keysound` / `composite`；缺省推演见 §8。 |
| `menu` | string | 统一框架 | 贴图右键「脚本」子菜单文本；`nil` 表示只出现在脚本页。 |
| `requires` | string[] | 注册表读、菜单过滤用（`ExtSchema::available`） | 这个脚本要用的**外置文件**（相对脚本目录）。缺任何一个 ⇒ **不进**贴图右键「脚本」（m13）。路径解析与 `ctx.fs`、`ext:proc` 共用**同一条路径判据**：绝对路径 / `..` 一律算不满足，不顺着找。空 = 不依赖任何东西（没声明的老脚本永远可见）。 |
| `hide_cursor` | bool | 引擎读取全局 | 脚本声明隐藏系统**箭头**时设 `true`（I 形/手形/缩放提示保留）；不存在 / 非布尔 → `false`。必须在 `load_script` 之后读取（顶层赋值在执行期写入 globals）。 |

---

## 8. 脚本必须 / 可以定义的回调（函数）

这些**不是** Rust 预定义函数，而是宿主在合适时机反向调用脚本里定义的全局函数。
所有 `params` 参数都是 `schema()` 声明的参数表。**m17 起宿主在交棒给脚本前把 `default` 合并进去了**：凡 `schema` 声明过的键，`params` 里**一定存在**（配置里没写过 = 交默认值），不必再写 `params.x or 默认`；用户显式填过的值（包括 `false` / `0` / 空串）原样交，声明之外的键也原样透传。也可读全局 `_params`。

| 回调 | 签名 | 被谁调用 | 说明 / 返回值契约 |
|---|---|---|---|
| `schema` | `schema() -> table` | 注册表扫描期 + 运行时 | **必需**（否则无参数 UI）。返回参数声明数组。 |
| `defaults` | `defaults() -> table` | 引擎 | **遗留兼容**：老脚本不写 `schema` 时回退用。新脚本请用 `schema`。 |
| `init` | `init()` | 各宿主 | 可选。启用 / 热重载后调一次，清零状态。缺失不报错。 |
| `on_move` | `on_move(x, y, t, params)` | 拖尾 / 悬浮标 / 点击效果 / 复合 | 鼠标采样。`t` = 共享时钟（秒）。 |
| `step` | `step(dt, t, params)` | 同上 | 每帧推进。**`dt` 单位 = 秒**；老化 / 淡出必须放这里（鼠标不动也要推进）。 |
| `draw` | `draw(ctx, params) -> number?` | 同上 | 每帧绘制。返回值语义 = 续帧请求：返回 `n>0` = "n 毫秒后再来一帧"；返回 0 / 负数 / 其它类型 / `nil` = 不请求额外帧。**只对悬浮标（`HoverDot/`）真正成立**；拖尾 / 点击不看返回值（靠"是否还在画"判活）。 |
| `on_press` | `on_press(x, y, t, params)` | 全部宿主 | 指针按下触发一次。**兼容老名 `on_click`**（先找 `on_press`，没有再回退 `on_click`，都没有则静默跳过）。 |
| `on_mouse_down` | `on_mouse_down(button, x, y, params)` | 全部宿主（优先于 `on_press`） | 鼠标按下，`button` ∈ `"left"/"right"/"middle"/"x1"/"x2"`（参数顺序刻意与 `on_press(x,y,t)` 不同名，防抄错）。取时间用全局 `elapsed`。缺失时回退 `on_press`。 |
| `on_mouse_up` | `on_mouse_up(button, x, y, params)` | 全部宿主 | 鼠标抬起。按住态视觉（加速/后坐/连发）用；缺失静默跳过。 |
| `on_wheel` | `on_wheel(delta, horizontal, x, y, params)` | 复合 / 拖尾宿主 | 滚轮。`delta` 一格 ±120（正 = 上/右），`horizontal` = 倾斜滚轮。**只有脚本声明它，钩子才转发普通滚轮**（否则每滚一格不唤醒 UI）；缺失静默跳过。 |
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
| `default` | number / string / bool | 默认值；缺失时按类型给兜底。★ 合并发生在**交棒给脚本那一刻**（`ext_runner::merged_params`），所以脚本收到的表里声明过的键一定在；界面的初值与脚本拿到的默认值是**同一个来源**（`ParamDef::value_or_default`，由 `ui/model.rs` 那格对拍钉住）。存的值类型与声明不符 ⇒ 回退默认值并记一行日志。`int` 档交的是**整数**（Lua 里 `math.type` 为 `integer`），不是浮点。 |
| `choices` | table(array) | `choice` 类型的候选项字符串（也是存进配置、喂给脚本的稳定值，勿改）。 |
| `labels` | table(array) | `choice` 各选项界面显示名（可选，与 `choices` 一一对应；长度不符则整份丢弃）。 |
| `group` | string | 分组名（UI 分组标题，可选）。 |
| `alpha` | bool | **仅 `color` 用，默认 `true`**。取色器带透明度条、落盘走 8 位 `#RRGGBBAA`、脚本 `hex_to_rgb` 第 4 返回值拿到 0–255 alpha；只有显式写 `alpha=false` 才退回 6 位（alpha 恒 255）。 |

**`type` 取值 → 面板控件**：`float`（滑条+输入框）/ `int`（整数滑条）/ `bool`（开关）/
`color`（取色器，默认带透明度条；`alpha=false` 关闭）/ `choice`（下拉）/ `sound`（音效下拉）/
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
| `platform/cursor_hint.rs` | `cursor_hover` 的判定：系统标准交互光标句柄白名单 |
| `platform/capture_exclusion.rs` | 桌面镜片期间把覆盖层排除出系统抓屏（`WDA_EXCLUDEFROMCAPTURE`） |
| `core/image.rs` | `lens_circle`：圆形蒙版 + 缩放/difference 的纯像素变换 |
| `scripting/fs_api.rs` | `ctx.fs` 只读文件访问（§5）+ 逃逸校验 |
| `scripting/registry.rs` | 特效三目录 Manifest 扫描 + `ParamType` / `ParamDef` / `EffectSchema` |
| `scripting/registry_ext.rs` | 统一框架（递归扫 `assets/lua/`）Manifest + `role` 推演 |
| `scripting/ext_runner.rs` | 贴图 / 脚本页扩展脚本执行器（`run(ctx,params)` + `ctx.image` / `ctx.hwnd` 注入） |
| `core/ui_bus.rs` | 进程级 UI 总线（脚本请 UI 线程开模态窗；存的是发送闭包，`core` 不依赖 windui） |
| `platform/process.rs` | **全仓唯一**起外部进程的落点（超时 / kill / 截字节 / 码页）；脚本侧入口是 `ext:proc` |
| `ui/script_dialog.rs` | 脚本模态窗本体（`ext:result` / `ext:input` 的宿主外观：两列 / 只读正文 / 输入框 + 复制） |
| `scripting/runtime.rs` | 宿主级 VM 缓存 `EngineCache`（热更新 + GC） |
| `modules/trail/companion.rs` | 复合脚本宿主：注入 `play_sound`、`host="panel"`、调用各回调 |
| `modules/key_sound.rs` | `keysound` 宿主：构造 `ctx.sounds/mods/key/repeated/dt` 并 `run` |
| `modules/trail/fx_core.rs` | `FxCore` 驱动（拖尾 / 悬浮标 / 点击效果 / 复合共用的引擎） |
