# assets 目录版图

**一个功能一个文件夹**，功能自己的脚本、声音、图片都放在它那一个文件夹里
（例外：`Sounds/` 是点击 / 键盘 / 拖尾共用的，`VirtualKeyboards/` 是平铺的方案文件）。
程序只按「功能文件夹名 + 文件名」定位资源，新增素材不用改代码布局。

| 文件夹 | 归谁 | 放什么 |
|---|---|---|
| `lua/` | 统一脚本 | **唯一 Lua 脚本根**（递归）。所有 role=effect/keysound/image/composite 的 `.lua` 都放这里（可按子目录整理） |
| `lua/scripts/` | 通用扩展脚本 | `role=image`（贴图右键「图片操作」）与 `role=composite`（复合脚本）。**文件名即 id**，
  菜单是否出现由 `runtime::Runtime::lua_ext_menu_items` 按 `role_is_active("image", id)` 过滤；
  出厂样例：`image_info` / `avg_color` / `composite_demo` / `unified_sound` / `class_sound` |
| `lua/MouseTrail/` | 鼠标拖尾 | 拖尾特效脚本，文件名即特效 id |
| `lua/HoverDot/` | 悬浮标 | 悬浮标脚本 + PNG 图片（`.png`，「PNG 图片」样式从这里取图，用户选的图也会复制进来） |
| `lua/ClickFx/` | 点击效果 | 点击效果脚本 |

> ★★ **挂在哪由目录决定**：上面四个目录 = 四个挂载点，**同一份 `.lua` 复制到哪个目录，
> 就在哪个下拉 / 卡片里生效**。脚本里没有任何字段能改挂载点，`role` 只用来区分
> 开关语义（`image` = 贴图右键菜单可见性、`keysound` = 键盘页、其余 = 常驻运行开关）。
> 反之，`role="image"` / `role="keysound"` 的脚本不会被鼠标下拉列出来。
| `Sounds/` | 声音素材（共用） | 音频文件（`.wav` 等），点击声音/键盘声音/拖尾音效的出厂音源共用本目录（2026-09-25 由 `ClickSound/` 改名），下拉数据源 = 目录扫描 |
| `KeyDisplay/` | 击键回显 | `keynames.ini` 键名映射表（改文件即改回显文本，保存后按一次键生效，无需重启）；键名 ID 清单见文件头注释 |
| `VirtualKeyboards/` | 虚拟键盘 | 出厂键盘方案 `.json`（**平铺**，一个文件一套方案，**文件名即方案 id**，规则 = `几何前缀_样式`：`ansi_104` / `ansi_104_dark` / `ansi_104_kb_double` / `ansi_104_kiosk_dark` / `ansi_104_vhand_dark` / `ansi_104_zh_light` 六套同几何，加 `ansi_60` / `wasd_gaming` 两套独立几何）；改出厂方案要同时改 `main/assets/VirtualKeyboards/*.json` 与配置里的默认方案名 |
| `icons/` | 全局 | 图标（不属于任一功能）。**源是矢量**：`icon.svg`（常规态，黑底 + 青 `#3df8f8` 图形）与 `icon_red.svg`（录制态，圆点红色；结构须与常规版一致）。产物 `tray_<N>.rgba` / `tray_<N>_red.rgba`（托盘五档 N=16/20/24/28/32 裸像素，按 DPI 挑档）与 `app.ico`（exe/窗口，12 档）**都由 `cargo run --example gen_icon --features icon-gen` 从 SVG 逐档光栅化生成，不要手改**。★ 命名跟源走 `red` 不叫 `rec`：区别是**颜色**，不是用途。改完 SVG 重跑该命令并重新编译 exe |
| `images/` | 关于页 | 赞助二维码：**源是矢量** `WeChatPay.svg` / `ALiPay.svg`（真码，模块铺满 viewBox、无静默区）。产物 `WeChatPay.png` / `ALiPay.png` 由 `cargo run --example gen_icon --features icon-gen` 渲染（额外补 4 模块白边作静默区），`ui/pages/about.rs` 用 `include_bytes!` **编译期嵌进 exe**；换二维码 = 覆盖这两个 SVG 再跑该命令并重新编译。★ 源用矢量是刻意的：它是编码数据，模块必须落在像素网格上，位图缩放/抗锯齿会降低识别率，而矢量可按需渲染到任意尺寸（高 DPI 也不糊）。布局（160px / 间隔 28 / 白框 2px / 说明字在图上方）照抄 Pecia 的 `AboutPanel::draw()` |

> `manual/` 目录放《隐藏快捷键手册》（`.md` 源 + `.html` 成品）：面向最终用户的随包文档，程序不读它、纯文档（2026-09-29 用户口径，跟随 assets 发布）。

## 两条约定

1. **只读不写**：程序运行时不往这里写任何东西（用户选的悬浮标图片是唯一例外，
   那是显式「导入素材」动作）。配置、日志、预设都在程序目录根部，与 assets 无关。
2. **打包要同步**：`build.rs` 不拷贝 assets，改动必须手动同步到 `build/assets/`，
   否则发布出去的版本读不到（脚本/音效/图片全部静默缺失）。
