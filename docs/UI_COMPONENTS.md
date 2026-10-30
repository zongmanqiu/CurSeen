# UI 组件与 Lua 脚本参考

> 面向"往面板加控件"和"写 Lua 脚本"的人。**改了组件库 / 参数类型 / Lua API 必须同步本文**。
> 组件正本在 `src/ui/card.rs`；脚本参数系统在 `src/scripting/registry.rs`（声明侧）与
> `src/ui/model.rs`（信号侧）；脚本 API 在 `src/scripting/api_*.rs`。
> 设计取舍见 `DESIGN.md`；踩过的坑见 `docs/踩坑记录*.md`（分册）。

## 一、面板行组件库（`ui::card`，唯一正本）

行组件 = 「标签列 + 控件列」的一行，全部基于 [`card::option_row`]：标签宽由调用方传的
`col1_w` 决定（每页用 `card::calc_col1_width` 按本页所有卡片名 + 参数标签里最长者算
**一个**基准，页内所有行严格同 x 对齐）。

**★ 对齐标准（2026-09-25 "统一 UI" 定案）**：标题栏控件列与正文参数控件列**同 x**：

```
控件列 x = 卡片左 + 16(行内边距) + col1_w(标签列) + body_indent(正文缩进)
```

`build_title_row` 会自动给标题栏下拉补一个 `body_indent` 宽的前导占位，两条等式因此
共用同一个偏移变量——页面要规划间隔，**只动 `build_title_row` 的 `body_indent` 一个
变量**，不再各自目测调间距。取值只允许两个（`0` 子卡 / `INDENT_CARD`=`SWITCH_W`，
`ui::card` 常量，护栏测试
`alignment_indents_are_tied_to_the_switch_width` 钉死）：

**★ 标题栏下拉右缘对齐（2026-09-25 第三定案，推翻同日更早的"拉伸填满"误案）**：
标题栏下拉宽度 = `card::CONTROL_GROUP_W`（= 数值滑条 120 + 间隔 8 + 数值输入框 72
= 200，单一来源常量），左缘同正文控件列（`body_indent` 占位保证），**右缘恰好落在
`slider_row` 数值输入框的右缘**——"不透明度"这类行的右缘就是标题下拉的右缘。
不要改成 weight 拉伸：`flex_spacer` 本身也是 `weight(1.0)`（会被对半分），且填满
整行与正文控件的右缘毫无关系（2026-09-25 用户纠正过一次）。显示名截断预算
`DROPDOWN_TEXT_W`（=158）由 `CONTROL_GROUP_W` 推导。

**★ 选择类正文控件同宽（2026-09-25 同日扩展，同日第五定案收拢为正本函数）**：正文里的**选择类**控件——效果参数
下拉（Choice/Sound）、取色器（Color）、点击声音五键下拉、击键回显四色——也取
`CONTROL_GROUP_W`：左缘同控件列，右缘与标题栏下拉是**同一条竖线**。内容宽不再自适应，
超长候选项必须自己按 `DROPDOWN_TEXT_W` 预算收 `…`（`card::ellipsize`；音效显示名在
构造处已截断）。例外：候选项长度不可控的**音频设备下拉**（capture 页"系统声音/
麦克风"）保持内容宽，不套本标准。

**★ 行助手唯一正本（2026-09-25 第五定案）**：所有参数行一律走 `card` 正本函数，
**页面不得自拼控件**——

| 正本函数 | 形状 |
|---|---|
| `card::option_row(label, control, col1_w)` | 通用底座（等高 `ROW_H`，标签列同宽） |
| `card::slider_row(label, sig, text, min, max, dec, col1_w)` | 滑条 + 数值输入框**三件套**（§35 起全软件唯一滑条样式） |
| `card::choice_row(label, Vec<String>, sig, col1_w)` | 下拉（宽 = `CONTROL_GROUP_W`；§37 起选项**不预截断**——闭合框超宽项由 windui `Dropdown::paint` 绘制时前置 `…`，弹出菜单按内容自适应变宽） |
| `card::color_row(label, sig, col1_w)` | 取色器（宽 = `CONTROL_GROUP_W`） |
| `card::browse_row(...)` | 浏览按钮 + 完整路径 |

tool / screen / capture 三页的私有 `choice_row` 已降级为对正本的**委托包装**（保持
`&[&str]` 调用面不变）；击键回显/虚拟键盘的方案参数行（`control_rows`）改走
`slider_row` / `color_row`，与鼠标页逐字节同款。以后改行样式只需要改正本一处。

| 取值 | 常量 | 用在哪 |
|---|---|---|
| `0` | — | **子卡**：且标题栏**不放假开关占位**（第四定案），子卡标题文字 = 父缩进 24 + 内边距 16 = **+40 对齐线** |
| `24` | `card::INDENT_CARD`（= `SWITCH_W`） | 普通卡**和含子卡的父卡**：正文参数标签与标题文字左对齐全靠这条等式 |

**★ 列宽全局唯一 `card::COL1_W`（2026-09-25 第六定案）**：此前六页各自
`calc_col1_width(本页标签)`，标签集不同 → 各页控件列 x 随页漂移（用户报"键盘页
标题下拉与鼠标页位置不一样"）。现收敛为常量 `COL1_W = 208`（容最长标签
"不按 Shift 印下档"，护栏测试 `col1_width_covers_the_longest_labels` 钉死），
六页全部 `let col1_w = card::COL1_W;`——**任何页、任何卡、任何行，标签列/控件列
x 完全相同**。`calc_col1_width` 保留仅供该护栏测试与将来调值时复算。页面出现更长
标签时改 `COL1_W` 一个常量。

⚠️ 余量（2026-09-27 二次）：设置页「快捷键」集合的名改成「模块 > 页内原文」后，最长项
「截图与贴图 > 截图并贴图快捷键」按渲染字号 `FONT_MD` 折 207.2px，只剩 0.8px；同日
「截图与贴图」三行省掉「快捷键」三字（→「截图」「截图后贴图」「贴图」），最长项落到
「截图与贴图 > 截图后贴图」165.2px，余量回到 42.8px。护栏
`hotkey_collection_labels_fit_the_column_at_render_font` 盯着这条线——再长一个字就
得动 `COL1_W`，而 `COL1_W` 一动，全项目的控件列 x 一起挪，必须全局验收。

**全项目只有一条文字对齐线 `卡左 + 40`**：主卡标题文字、主卡正文标签、子卡标题文字、子卡正文标签全部落在它上面；
主卡与子卡的标题栏下拉也同 x（父缩进 24 恰好补偿子卡 body_indent 0）。子卡因此
左右各内收 24px（嵌套观感）。

| 组件 | 签名要点 | 用途 / 注意 |
|---|---|---|
| `option_row(label, control, col1_w)` | 任意控件 | 一行的骨架：标签 + 控件 + 弹性空隙，行高 `ROW_H`、交叉轴居中 |
| `option_row_signal(label, control, col1_w)` | `label: Signal<String>` | 同 `option_row` 版式，但**行名随信号联动**（如「系统」卡的四态「管理员权限 (未启用)/(重启后有效)/(重启后取消)/(已启用)」）——状态直接写进行名、不另配说明行的场景用这个 |
| `slider_row(label, sig, text, min, max, decimals, col1_w)` | 两个信号：`sig: Signal<f32>`（比例）+ `text: Signal<String>`（镜像文本） | **标签+滑块+72px 数值输入框三件套**（§35 定案：全软件**所有**数值条一律这个样式，原无输入框版本与 `slider_input_row` 已合并进本函数）。滑块不带值标签（数值只在输入框一处）。详见第二节 |
| `browse_row(label, sig, col1_width, display, act)` | `sig: Signal<String>` | 标签+「浏览…」按钮+路径（`path_display` 补全、超宽 `ellipsize` 收 `…` 保尾部） |
| `hotkey_row(label, sig, kind, capture, col1_w)` | 快捷键框（宽 = `CONTROL_GROUP_W`，§36 起与输入类控件同一条右缘竖线） | 点击进入捕获态，按键写入快捷键串；**失焦即放弃捕获**（2026-10-02 口径：点框外任何地方 → 退出捕获态、显示回「（空）」，经 windui 补丁 094 的 `on_blur`，回调里判「捕获目标仍是本框」才清）；空串 = 停用，框内显示「（空）」（2026-09-27 口径，原「留空停用」四个字像动作文案） |
| `hotkey_row_script(label, sig, target, cap, col1_w)` | 同上，捕获目标 = `ScriptHotkeyCap`（2026-09-29，Lua `type="hotkey"` 参数） | 与 `hotkey_row` **共用视觉正本 `hotkey_box_el`**（框宽/高/配色/圆角/字号只此一份）；捕获态走 `PanelModel::script_hotkey_cap`，回写与互斥（固定字段 + 其它脚本热键，normalize 同口径）在 `set_script_hotkey_exclusive` 一处 |
| `divider()` | — | 卡片内分隔线 |
| `group_label(text)` | — | **段名**（2026-10-07）：同构的两组行并进一张卡时，标出每段是什么（如「功能开关」卡里的「模块开关」「脚本开关」）。与它上面的 `divider()` 配套；不带标签列宽，不参与控件列对齐 |
| `FeatureCard::new(name, expanded)` | 链式 | 卡片：`.switch(sig)` 标题栏开关、`.title_dropdown`/`.title_dropdown_signal` 标题栏下拉（自动带 `indent` 宽对齐占位，宽度 = `CONTROL_GROUP_W` 右缘对齐数值输入框，见上方"对齐标准"）、`.title_extra(el)`、`.body(el)`、`.build_title_row(col1_w, indent)`。展开态信号来自 `m.expand(id)`（落盘，id 见 `core::config::CARD_IDS`，**搬家不换 id**） |
| `calc_col1_width(names, font_size)` | — | 列宽计算（含中英混排宽度模型 `text_width`） |

★★ **快捷键的两条链路（2026-10-01 用户口径："脚本的开关和模块开关是分开的，脚本是不是
也该有专门的快捷键卡片"）——是。** 固定字段那套（`card::hotkey_row` + `HotkeyField`）在
设置页「快捷键」集合卡里全量列出，行名 =「模块 > 页内原文」；脚本的键位是**另一条**持久化
链路（写在脚本自己的 `hotkey` 参数里，见下表 `hotkey` 行），由设置页**「脚本快捷键」镜像卡**
按**脚本显示名**列出。两处的收录与互斥早已打通（`set_hotkey_exclusive` 与
`set_script_hotkey_exclusive` 双向清空对方同值绑定），只是界面上脚本热键原先只在「脚本」页
逐卡可见。「脚本快捷键」卡不新增任何逻辑：行内仍绑脚本参数自己的 sig，捕获态仍走
`script_hotkey_cap`，参数整批重建时行跟着重建；**没声明 `hotkey` 参数的脚本不画空行**。

★ 该卡的行名是「**脚本名 > 参数名**」（2026-10-01 用户口径，两段都用现成文本：脚本
显示名 `LuaExtCard::name` 与参数单元自己的 `label`，例如「AI 写脚本 > 快捷键」），
与上一张卡同样是「模块 > 行标签」两段式，好处是设置页这一行与「脚本」页那一行能对上号。

★★ **屏幕标注工具的顺序只有一处正本**：`core::hotkeys::DRAW_TOOL_ORDER` = 工具栏按钮
前三组的扁平序（画笔1/2/3 → 直线/箭头/矩形/椭圆 → 文字/序号/马赛克/**橡皮擦**），
三处照抄：工具栏分组 `toolbar::GROUPS`、`screen` 页「快捷键」子卡的行序、设置页集合表的
行序。出厂键位不变（1/2/3/5/6/7/8/9/0/4，序号留空），改的只是**界面行的排布顺序**。

**量纲约定（全线适用，踩过三次的坑）**：windui `Slider` 内部恒存 `0..=1`；信号里装的
是比例，配置里装真实量纲，两端靠 `ui::model::{v2s, s2v}` 抵消。`min`/`max` 必须与信号
初始化用的是**同一对常量**（量程单一来源 = `core::config`），错位的表现是"拖了没反应"。
浮点参数按 `step` 吸附走 `ui::model::quantize`（吸附结果回写滑块，读数与生效值同源），
落盘过 `json_decimal`（否则 1.4 落成 `1.399999976158142`）。

## 二、「滑条 + 输入框」三件套（`card::slider_row`）

解决"滑条点不精确"（例：时长 1–3600 秒量程 1px ≈ 18s）。输入框 72px、
`autofocus_select_all`（点击全选直接输）、键入实时生效。

**两个信号**：滑条 `Signal<f32>`（比例）+ 输入框 `Signal<String>`（镜像文本）。
写回侧按"**谁与基准（配置现值）偏离谁赢**"对账，三处实现、规则一致：

| 场景 | 实现点 |
|---|---|
| 面板固定参数（§35 起全部 25 根：偏移/字号/音量/倍率/透明度…） | `ui::model::slider_pair_sync`（配置现值当裁判，`apply_to` 各滑条块调用） |
| 脚本参数（Float/Int，schema 驱动） | `ui::model::float_param_sync`（`stored[key]`，无键回退 `initial()`） |
| 计时器时长（u64 钳制特例，先于 helper 落地） | `PanelModel::apply_to` 的计时器时长块 |

对账规则（两处同款）：

1. 滑条值 ≠ 基准 ⇒ 滑条刚拖：值跟滑条（吸附网格），文本回写成新值；
2. 文本是合法数字且 ≠ 滑条刻度（此时滑条值必 = 基准）⇒ 文本刚编辑：值跟文本
  （同样吸附、钳量程），滑条跳过去，**文本不回写**（前导零等中间态原样保留）；
3. 空串 / 非数字 / 溢出 ⇒ 打字中间态：忽略，值跟滑条，也**不**把文本回写成旧值
  （清空重输不被同步进程打架）；
4. 滑条原始位置不在网格上时恒吸附回写（读数与生效值同源，幂等）。

**测试要求**：新增一个用该三件套的参数，必须同时进
`every_real_valued_slider_roundtrips`（量纲往返）并配一条对账测试（参照
`timer_duration_text_box_and_slider_stay_in_sync` / `script_param_text_box_and_slider_stay_in_sync`
的四条路 + 收敛断言）。

## 三、脚本参数 schema → 面板控件映射（`registry::ParamType`）

脚本在 `schema()` 返回的表里逐条声明；宿主据此自动造 UI、自动取值（`EffectSchema` →
`EffectParamCell` → `trail::build_param_cell` 渲染）。字段（除 `key/label/type` 外均可选）：

```lua
{ key = "size",  label = "大小",  type = "float", min = 1, max = 100,
  step = 0.2, default = 12, group = "形状" }
```

| `type` | 控件 | 值语义（落盘 & 喂给 `params`） | 备注 |
|---|---|---|---|
| `float`（默认） | 滑条+输入框三件套 | number，按 `step` 吸附 | `step` 决定吸附网格与小数位；未写默认 0.01 |
| `int` | 滑条+输入框三件套 | 整数 number | 显示/落盘都是整数 |
| `bool` | 复选框 | bool | |
| `color` | 取色器 | `#RRGGBB` / `#RRGGBBAA` | **默认带透明度条**（8 位 **`#RRGGBBAA`**，alpha 在末尾）；只有显式 `alpha = false` 才退回 6 位不透明 |

> ★★ **颜色串的唯一口径：`#RRGGBB` / `#RRGGBBAA`（alpha 在末尾）。**
> 与 windui `Color::to_hex_string`、`config.json`、Lua `hex_to_rgb`、配置里的
> `[R,G,B,A]` 数组四处完全一致。项目里**不存在** `#AARRGGBB`（alpha 在前）——若某处按"alpha 在前"解析，
> 脚本 `default = "#FF000040"`（红 + 25% 透明）就会被读成不透明深蓝、
> `"#00FFCCBF"` 被读成全透明。
> 内存里的 `Argb(u32)` 仍是 `0xAARRGGBB`（Win32/GDI 要求），但**只在 `core::color`
> 内部**存在，字符串一律经 `Argb::parse_hex` / `to_hex_rgba` 转换。
| `choice` | 下拉 | `choices` 里的稳定标识（英文 id） | `labels = {…}` 给中文显示名，长度不符整份忽略 |
| `sound` | 下拉（宿主扫描） | 素材短名或绝对路径 | 候选由宿主按音效目录扫出，脚本写不出 |
| `folder` | 浏览（目录） | 目录路径，空串 = 出厂目录 | 改动会使同脚本 `sound` 参数整批重建 |
| `file` | 浏览（音频文件） | 文件名（复制进 `assets/Sounds/`） | 便携：素材跟着程序走（★ `assets/KeySounds/` 已于 2026-09-25 退役，见 `assets/Sounds/README.md`） |
| `str` | 文本输入框 | 原样字符串 | api_key / 模型名 / 任意文本 |
| `image` | 路径输入框 | 图片标识 | 悬浮标 PNG 类型在用 |

> 未识别的 `type` 按 `float` 处理。参数落盘键：拖尾特效 = `{特效id}.{key}`（共用一张表），
> 其余（悬浮标 / 点击效果 / 击键音效 / Lua 扩展）= 各自一张表、直接用 `key`。
> "控件没动过就不落盘"由 `apply_param_value` + `initial()` 保证（勿破坏，见其注释）。

## 四、Lua 脚本契约（四类脚本）

> 契约正本 = `assets/lua/README.md`（面向脚本作者，含示例与排查表）；本节是面向
> 面板开发者的摘要，两处以正本为准。

★★ **归属由目录决定**（2026-09-28 定稿，脚本里没有任何字段能改挂载点）：统一注册表
**只扫 `assets/lua/`**（递归），`MouseTrail/` `HoverDot/` `ClickFx/` `scripts/` 四个目录
= 四个挂载点，**同一份脚本复制到哪个目录，就在哪个下拉 / 卡片里生效**。别往脚本里加
"声明挂载点"的字段——加了也不会被读，只会误导脚本作者。

脚本顶层声明元信息：

```lua
effect_name = "实心圆"    -- 绘制特效（role="effect"）：下拉框显示名；name 优先，二者互为别名
name = "图片信息"         -- run 类扩展脚本：卡片显示名；无则用文件名
description = "…"
kind = "trail"           -- 可选：包卡下拉的类别标签（cursor/trail/click/hover/other）
role = "effect"          -- 可选：四个挂载目录下缺省由目录推断（effect/composite）；
                         -- ★ 只决定开关语义 / 菜单归属，**不改挂载点**（那是目录的事）
menu = "图片信息"         -- role=image 时：贴图右键菜单文本
hide_cursor = false      -- 声明隐藏系统箭头（如激光笔）；I 形/手形/缩放提示保留

function schema() return { … } end   -- 第三节的参数声明（可选）
function run(ctx, params) … end      -- keysound / image 扩展的统一入口
```

### 4.1 绘制特效（拖尾 / 悬浮标 / 点击效果；`role="effect"`）

生命周期回调（全部可选；按帧驱动，**绘图指令只能在 `draw` 里发**）：

| 函数 | 时机 |
|---|---|
| `init()` | 特效加载 / 参数变更后 |
| `on_move(x, y, t, params)` | **拖尾 / 悬浮标 / 独立卡片**：指针真动了才发；**点击效果**：实现了 `on_move` 的脚本走常驻通道（每个真实采样都发），只写 `on_press` 的脚本维持"按下出生" |
| `on_press(x, y, t, params)` | **全部宿主**：鼠标按下；老名 `on_click` 仍被识别（自动回退），新脚本优先写 `on_mouse_down` |
| `on_mouse_down(button, x, y, params)` | **全部宿主**：鼠标按下；`button` = `left/right/middle/x1/x2`；缺失回退 `on_press` |
| `on_mouse_up(button, x, y, params)` | **全部宿主**：鼠标抬起；按住态视觉（加速/后坐/连发）用 |
| `on_wheel(delta, horizontal, x, y, params)` | **复合 / 拖尾宿主**：滚轮。`delta` 一格 ±120（正 = 上/右），`horizontal` = 倾斜滚轮；只有脚本声明它，钩子才转发普通滚轮 |
| `step(dt, t, params)` | 每帧逻辑；`dt` 四类宿主**一律是秒**，固定步长（1/120 秒）子步进 |
| `draw(ctx)` | 每帧绘制；`ctx` 是 **DrawList userdata**（冒号调用，见下） |

全局变量（**四类宿主同一套**，每帧更新）：`cursor_x` `cursor_y`（指针位置）、
`cursor_vx/vy/speed/heading`（60fps 每帧像素 / 弧度）、
`mouse_vx/vy/speed/heading`（CursorFX 别名）、`mouse_down`、`mouse_button`、
`cursor_hover`（系统正在给出交互光标提示：I 形/手形/缩放/禁止/帮助/十字；
桌面按钮仍是箭头，所以它不代表“指针下有按钮”）、
`origin_x` `origin_y`（实例出生点）、`elapsed`（秒，动画相位用）、
`wall_ms`（系统运行毫秒，休眠期间也推进）、`local_ms`（本地 Unix 毫秒，时钟用）、
`unix_ms` / `tz_offset_min`、`screen_w` `screen_h`、`screen_x` `screen_y`（虚拟桌面原点，
可为负：整屏暗罩/星空/镜像中心必须用它）、
`host`（`"trail"`/`"hover"`/`"clickfx"`/`"panel"`）、
`_params`（当前参数表）、`ctx`（DrawList）；`mouse_x`/`mouse_y` 是 `cursor_*` 的别名。

**绘图指令**（`ctx:命令(...)`；颜色一律 `r,g,b,a` 四个 0–255）：

| 命令 | 签名 | 说明 |
|---|---|---|
| `circle` | `(x, y, radius, r, g, b, a)` | 实心圆（SourceOver） |
| `ellipse` | `(x, y, rx, ry, r, g, b, a)` | 填充椭圆 |
| `ellipse_ring` | `(x, y, rx, ry, width, r, g, b, a)` | 椭圆描边 |
| `round_rect` | `(x, y, w, h, radius, r, g, b, a)` | 填充圆角矩形 |
| `round_rect_ring` | `(x, y, w, h, radius, width, r, g, b, a)` | 圆角矩形描边 |
| `plus_circle` | 同上 | 加色（Plus）实心圆，辉光层用 |
| `glow` | 同上 | 三层加色叠出的辉光（半径×2.5/×1.2/×1） |
| `radial` | 同上 | 中心色→边缘透明的径向渐变圆 |
| `radial_stops` | `(x, y, radius, stops)` | 多段径向渐变；stops=`{{offset=0,r=..,g=..,b=..,a=..},…}` |
| `backdrop_circle` | `(x, y, radius, mode, zoom, r, g, b, a) -> bool` | 真桌面采样：`"magnify"` 放大镜 / `"difference"` 逐通道反色；`a` 0–255；返回 false = 抓屏不可用。运行期间覆盖层不进系统截图/录屏 |
| `clear_circle` | `(x, y, radius)` | 挖洞：圆内清成全透明（颜色忽略传 0） |
| `ring` | `(x, y, outer_r, width, r, g, b, a)` | 圆环，width 从外径向内收；width ≥ outer_r 退化为实心圆 |
| `rect` | `(x, y, w, h, r, g, b, a)` | 矩形 |
| `line` | `(x1, y1, x2, y2, width, r, g, b, a)` | 圆头圆角描边线 |
| `polygon` | `(points, r, g, b, a)` | 填充多边形（SourceOver）；`points = {{x,y},…}` ≥3 点 |
| `fill_polygon` | 同上 | 加色版多边形（飘带） |
| `diamond` | `(x, y, size, r, g, b, a)` | 菱形（加色） |
| `diamond_rot` | `(x, y, s, angle, r, g, b, a)` | 旋转菱形（加色），粒子核心 |
| `image` | `(x, y, scale, a)` | 悬浮标 PNG 贴图（宿主预载，(x,y) 为中心，等比缩放） |
| `layer` | `("base" / "normal" / "top")` | 本脚本**后续所有**指令的绘制层；破坏性擦除（`clear_circle`）必须垫底 |

**助手函数**：`hsv_to_rgb(h,s,v) → (r,g,b)`、`hex_to_rgb("#RRGGBB"|"#RRGGBBAA") → (r,g,b,a)`
（a：6 位串恒 255）、`rng(min,max) → float`、`rng_int(min,max) → int`（区间非法退化为
`min`，不 panic）、`clamp(v,lo,hi)`、`lerp(a,b,t)`。

### 4.2 击键音效（`role="keysound"`）

入口 `run(ctx, params)`；`ctx = { role="keysound", sounds={文件名,…}, key, mods={ctrl,alt,shift,win}, repeated, dt }`
（另有 `fs`/`ext`，见 4.4）。**返回值语义**：返回 `ctx.sounds` 里的文件名/索引 → 播放它；
`false` → 强制静音；`nil` → 落回默认。`on_key(key, mods, repeated, params)` 是兼容旧式
音效脚本的第二入口，语义同返回值。

### 4.3 脚本扩展（`role="image"`）

贴图右键菜单（`menu` 声明菜单文本）→ `run(ctx, params)`；
`ctx = { role="image", image=?, hwnd=? }`（有贴图上下文时注入；`hwnd` = 目标窗口句柄）。
`image` 对象（**只读快照**，改动不回写贴图）：

| 成员 | 说明 |
|---|---|
| `.w` / `.h` | 只读尺寸（像素） |
| `pixel(x, y)` | → `r, g, b, a` 四个值；越界返回全 nil |
| `bytes()` | → 原始 RGBA 缓冲（二进制字符串，长度 = w×h×4，行优先）；整图统计用它 + `string.byte` 批量读 |
| `png_base64()` | → base64 编码的 PNG 文本（在线接口用） |
| `save(path)` | → 存成 PNG 文件（父目录不存在则创建） |

返回值：`run` 返回字符串 → 自动弹可复制的结果框（**多行文本直接用 `\n`**，宿主会
归一化成 EDIT 控件要的 `\r\n`）；返回空/nil = 脚本已自己 `ext:result`。
内置扩展脚本在 **`assets/lua/scripts/`**（通用扩展脚本目录，id = 相对 lua 根的路径去扩展名）；
击键音效脚本在键盘页专属的 **`assets/lua/KeySound/`**：`KeySound/unified_sound.lua`、
`KeySound/class_sound.lua`。
**AI 写脚本 / 图片体检 / OCR 文字识别 / 条码识别已迁到 `CurSeen-Scripts` 仓库**
（2026-10-30，作为在线包安装）：AI 包 `scripts/AI/ai_chat.lua` 连 `ai_chat_api.txt` 一起装；
OCR 与条码包自带官方工具（源码在仓库 `tools/ocr`、`tools/barcode`，编译产物不进仓库），装包即用。
复合示例 `3.composite_demo.lua` 已按用户口径删除，`tests/fixtures` 留了测试夹具。
守门测试见 `tests/script_pack.rs` 与 `repo_image_report_runs_with_image_context`。

★ **与模块特效脚本（`MouseTrail/` 等）的关系**：共用同一引擎、参数机制与驱动实现
（`FxCore`，见 `modules/trail/fx_core.rs`），**同一份脚本放哪个目录效果都一样**——差别只在
入口与生灭时机：特效脚本被各自模块的渲染循环逐帧驱动（拖尾采样流 / 悬浮标 / 点击效果），
扩展脚本走 `run(ctx, params)` 一次性调用，按 `role` 拿不同 ctx（`image`=贴图快照、
`keysound`=击键事件）。把特效脚本复制进 `scripts/` 也能跑（脚本页出卡，开关=常驻运行）；
反过来 `role="image"` / `"keysound"` 的脚本不会进鼠标下拉（各有归宿，被排除）。

### 4.4 通用 `ctx` 成员（所有 run 类脚本）

- `ctx.fs`：**只读**文件接口，相对脚本目录解析（`read / read_bytes / exists / list`）；
  `../` 与绝对路径返回 nil（逃逸校验，防手滑）。
- `ctx.ext` / 全局 `ext`：`ext:get_image()`（截屏）、`ext:log(...)`、`ext:http(...)`
  （返回 `status` / `body` / `result`）等宿主扩展。

### 4.5 复合脚本（`role="composite"`，API 全开放）

**定位**（2026-09-26 用户口径）：拖尾/悬浮标/点击效果/击键音效本身就是 Lua 脚本实现
的——把「跑这些脚本」的能力原样开放给 `scripts/` 里的用户脚本。一个复合脚本可以
**同时**实现拖尾 + 点击声音 + 悬浮标 + 任意键鼠反应，不借用内置功能；多个复合脚本
可同时运行（「同时开 3 种悬浮标」= 写 3 个复合脚本全部开启）。宿主 =
`modules::trail::companion`（常驻实例，运行列表存 `config.lua_ext.composite_active`，
参数仍存 `params[id]`）。脚本页列出这类卡片，标题开关 = **运行/停止**。

脚本卡排序（2026-10-29）：卡片先按可选 `category` 分组——`mouse` → `picture` → `text` → `ai`，
未知分类随后、**没有 `category` 的老脚本最后**（组内仍按原 id 顺序）。分类只影响排序，
不改显示名、不改 id；包卡从 `pack.json` 的 `category` 读，单脚本从顶层 `category = "…"` 读。
脚本页**没有刷新按钮**（m35 定稿）：宿主用 `ReadDirectoryChangesW` 监听**整个 `assets/`**
（`platform/dir_watch`，空闲零 CPU；2026-10-29 从 `assets/lua` 扩大），目录一变只置一个脏标记，
由 250 ms 对账在 UI 线程上重扫：脚本卡、拖尾/悬浮标/点击效果下拉、声音与键音列表、
虚拟键盘方案都在四分之一秒内自动更新，**不动配置与正在运行的实例**（新脚本按配置现值
初始化卡片，要跑还得自己勾开）。选中项按 id 保留，不因新增文件静默换效果。
贴图右键「脚本」子菜单本来就**每次右键重扫**。

回调（全部可选，签名与特效脚本一致的部分直接复用）：

| 回调 | 时机 |
|---|---|
| `init()` | 启用/热重载后 |
| `on_move(x, y, t, params)` | 鼠标采样（含快移插值点，与轨迹特效同密度） |
| `on_mouse_down(button, x, y, params)` | 鼠标按下；`button` = `left/right/middle/x1/x2`（★ 与波纹的 `on_click(x,y,t,…)` 刻意不同名，防抄错参数位） |
| `on_key(key, mods, repeated, params)` | 键盘按下；键名与击键回显同源 |
| `step(dt, t, params)` | 固定步长推进 |
| `draw(ctx, params)` | 每帧绘制；`ctx:circle / ring / rect / layer("base"/"top")` 等全套绘图 API |

全局：特效脚本全套（`screen_w/h`、`elapsed`、`mouse_x/y`、`rng`、`rng_int`、`clamp`、
`lerp`、`hex_to_rgb`）＋ **`play_sound(file, volume?)`**（解析链与击键音效「音效文件」
行同源：绝对路径 / 便携相对 / `assets/Sounds/` 裸名；返回是否真的发声）。

帧规则与特效一致：只有 `draw` 每帧真的产出指令才续帧（空闲 0% CPU）；要持续动画就
在 `draw` 里画。脚本文件改动按 (mtime, 字节数) **热重载**（状态清零、参数重放）；
脚本缺失/损坏时该实例回调静默跳过，不拖垮其它实例。复合脚本模板：
`tests/fixtures/lua/scripts/composite_demo_fixture.lua`（拖尾 + 点击音 + 点击效果三合一，
守门测试 `composite_script_runs_even_when_trail_disabled` 用真文件跑通端到端；颜色参数带
透明度条 = schema 里 `alpha = true`，默认 `#FF000040`）。

**脚本默认态（2026-09-27 定稿，按 role 分列表）**：两个 role 的出厂态一律"全关"
（缺键/空列表 = 关），用户去脚本页勾开要用的那几个。判定与写回只有
`role_is_active` / `set_role_active` 两个入口，不许再出现第三张表。

### 4.6 包卡（`scripts/<Pack>/`，2026-10-26 起）

`scripts/` 的**顶层子目录 = 一张包卡**，目录内**平铺的直接 `.lua` = 标题栏下拉项**；
下拉选中哪一项，下面就显示该子脚本的 `schema()` 参数。子脚本仍是普通脚本（可复制到
`MouseTrail/`/`HoverDot/`/`ClickFx/`），包卡只是 UI/运行容器。

- `pack.json`（可选）：`name` / `version` / `license` / `default` / `effects[]`（顺序与白名单）。
- 同一时间只跑当前选中的**一个**子脚本；开关 = 启动/停止它。
- **切换子脚本 = 恢复该脚本的 schema 默认**：先清掉本包全部旧参数，再按空表重建 UI；
  只有“当前脚本”的设定会落 `lua_ext.params`（与内置鼠标特效卡「切换即刷新」一致）。
- 实际运行 id = `scripts/<Pack>/<file>`，存 `composite_active`；下拉选择存
  `lua_ext.pack_selected[pack_id]`; 停在 `default` 时不落盘（零冗余）。
- 同名子脚本（如 `bubbles` / `stars` 各有点击与拖尾版）文件名可加后缀，但 `name` 保持原名；
  下拉只对重名项追加类别括号。
- **每个 role 都有开关**：语法同一形状、判定同一
入口（`LuaExtSettings::role_is_active` / `set_role_active`），只有落点不同：

| role | 开关写哪 | 勾上是什么效果 |
|---|---|---|
| `composite` | `config.lua_ext.composite_active` | 起一个常驻实例（见 §4.5） |
| `image` | `config.lua_ext.image_active` | 贴图右键「脚本」里出现这一条 |

- `image` 这一族是**手动动作**：勾上 = 菜单里出现**这一条**，关掉 = **这一条**收起
  （不是整个「脚本」消失，见下一条）。过滤点只有 `runtime::Runtime::lua_ext_menu_items`
  一个——`reg.scan()` 之后连过两道：`role_is_active("image", id)`（勾没勾）**与** `s.available()`
  （脚本声明的 `requires` 外置文件在不在，m13）。**"过滤"这件事仍然只有这一处**，
  两道都只是这一个点上的判据；别在 `scan()` 里丢弃条目（脚本页那列复选框要用 `list()`），
  也别在 `pin.rs` 拼表时再判一次。
- ★ 一句话记住历史：**手动动作 ≠ 自动运行，两者不许共用一张闸。** `image` 的开关只管
  "入口存在性"，从不喂常驻实例；反过来常驻实例的运行状态也影响不到菜单。
- 子菜单**恒挂载**——「脚本」这一级不会因为没脚本就消失；只有**里面有没有条目**
  同时看磁盘扫描与 `image` 开关。一条都没勾开时挂一条置灰的「（暂无脚本）」，
  而不是把整个入口撤掉（撤掉 = 用户以为功能坏了 / 入口随配置漂移）。

## 五、给面板加新组件的固定流程

1. **行组件** → `src/ui/card.rs` 加正本（带"为什么"的文档注释），页面删私有拷贝改调
   `card::`；**布局变化**（标签列宽、行高、间距）只动 `option_row` 一处。
2. **信号侧** → `ui::model.rs`：新信号 + `from_config` 初始化 + `apply_to` 写回
   （对账语义见第二节；量程常量进 `core::config` 单一来源）。
3. **测试** → 量纲往返（`every_real_valued_slider_roundtrips`）+ 对账/无幽灵写入
   （`model_no_spurious_change` 自动覆盖 + 专项测试）。
4. **文档** → 本文相应小节同步；`DESIGN.md` 只记设计级结论；踩到的坑进 `docs/踩坑记录*.md`。

## 六、字号层级（`ui::tokens`，两档封顶）

2026-09-28 用户口径："常用一种、最多三种，字号统一才方便、视觉上也不突兀"。于是**界面
里只有一种字号**：

| 档位 | px | 用在哪 | 约束 |
|---|---|---|---|
| `FONT_MD` | 14 | **界面里的一切文字**：侧边栏导航、卡片标题条、页内分区标题、行名、选项名、数值、链接、卡内说明，以及屏上的录屏完成提示窗 | 与 windui 默认 `font_size = 14.0` 同值（显式写出，不依赖默认值） |
| `FONT_DISPLAY` | 30 | 关于页 `CurSeen` 大标题 | 仅此一处 |

三条判据：

- **"放不下"是长度问题，不是字号问题**：windui 的文本**不换行**、超宽只会画出去，
  所以长文本一律按像素预算截断（`card::ellipsize`，与下拉显示名同一套宽度模型）。
  ★ 曾为录屏提示窗的长路径留过 `FONT_SM`(12) 这一档，实测同一条 46 字符的目录按
  12px 排仍要 391px，照样超出 372px 的可用宽——降档只是把溢出门槛推远，却凭空多出
  一档字号。已废，回归锁是 `rec_toast_elides_the_path_instead_of_shrinking_the_font`。
- **层级用颜色/底色/缩进表达，不用字号**："这是次要信息"首选 `TEXT_SECONDARY` 灰色。
  2px 的字号差看着像排版失误，层级也没更清楚；卡片标题与正文同字号时，靠黑 vs
  灰、卡片标题条底色和缩进来区分。
- **不提供"界面字号"用户设置**：面板是 DPI aware 的（windui 按 `dpi_scale()` 把逻辑
  坐标放大到物理像素），系统缩放时整个面板等比放大，手动开关是重复功能；而代价是
  列宽、截断预算、行高、侧边栏宽度这些按 14 算出的常量要全部改成可缩放。

★ 改 `FONT_MD` 会牵动：`card::COL1_W`(208) 的护栏测试
`col1_width_covers_the_longest_labels` / `hotkey_collection_labels_fit_the_column_at_render_font`、
`nav::NAV_WIDTH`(66，下限由"2 字 × 字号 + 内边距 13×2 + 指示条 3"算出，现为 57)。
