# CursorX（CurSeen Lua 扩展包）

**参考与致谢：** [CursorX — Interactive Cursor Effects](https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects)（MIT）。

本目录是 CurSeen 对 CursorX 效果的**独立 Lua 重实现**，用于学习和扩展 CurSeen 的鼠标特效，不是 CursorX 的官方移植，也没有复制上游的源代码、图片、音频或其它素材。上游 CursorX 的版权归其作者所有；这里提到它，是为了致谢与说明效果来源。

## 内容

共 24 个光标效果（与上游站点注册的 24 个一致）：

Magnetic、Particle Trail、Spotlight、Morphing Blob、Pixel Shatter、Elastic Ring、
Neon Glow、Text Orbiter、Gravity Pull、Constellation、Fire Trail、Crosshair Scope、
Mirror Ghost、Rainbow Comet、Bubble Float、Ripple Wave、Glitch Shift、Wind Stream、
DNA Helix、Torch Light、Difference Blend、Ghost Trail、Audio Pulse、Fluid Glass。

上游源码里另有 5 个**未在站点注册**的旧效果（MatrixRain、Clock、EyeTracker、
InkSplatter、RepelField），本包不收录：它们没有参数面板、不在展示序列里，其中
Clock / InkSplatter 的效果与 CurSeen 现有脚本重复。

## 使用

`assets/lua/scripts/CursorX/` 是**一个包**：目录本身在「脚本」页显示为**一张卡**，
目录内平铺的 `.lua` 是卡片标题栏下拉的选项。选择某个子脚本后，下方参数区会切换成
该脚本的 `schema()`。

- `pack.json`：包名、版本、许可证、默认项、下拉顺序。
- 子脚本可以单独复制到 `assets/lua/MouseTrail/`、`HoverDot/`、`ClickFx/`，由目标目录决定触发方式。
- 同一时间只运行下拉中选中的一个子脚本。
- 所有脚本都声明 `hide_cursor = true`：只隐藏系统**箭头**，I 形 / 手形 / 缩放提示保留。

### 换色类参数

`Neon Glow` / `Morphing Blob` 点击会在「参数颜色 + 6 个预设色」之间循环；修改颜色参数
会**立刻**回到参数颜色，不需要再点几次轮回去。

## 悬停态 `cursor_hover`

原库用 `elementsFromPoint` 判断指针是否在按钮/输入框/链接上。桌面环境没有 DOM，
CurSeen 改为读取**系统交互光标**（I 形 / 手形 / 缩放 / 禁止 / 帮助 / 十字）：这些
才是系统明确给出的“这里可以操作”的信号。桌面按钮通常仍用箭头光标，因此本包不会
把“悬停在按钮上”识别为悬停态——这是能力边界，不是遗漏。

## 桌面采样（Difference Blend / Fluid Glass）

这两个效果需要读取指针下方的真实桌面：

- `Difference Blend` 逐通道取 `|桌面 − 球色|`，复刻 CSS 的 `difference` 混合；
- `Fluid Glass` 把桌面按倍率放大成一枚圆形透镜。

宿主为此提供了 `ctx:backdrop_circle`。抓屏期间 CurSeen 会把**覆盖层自身**从系统
捕获里排除，否则镜片会把自己上一帧抓进源图（无限套娃）。代价是：**这两个效果运行
期间，覆盖层不会出现在任何系统截图/录屏里（包括 CurSeen 自己的截图）**；切换走后
约 1 秒自动恢复。Windows 10 2004 之前的系统不支持该排除，脚本会退回静态外观。

## 已知简化

- `Magnetic` / `Gravity Pull`：上游用 JS 拖拽**网页元素**；桌面没有可操作的 DOM，
  本实现把磁力/引力落在指针周围的铁屑、碎屑粒子上，保留原名与参数语义。
- `Text Orbiter`：渲染层没有旋转文字，字符沿轨道公转但保持正立。
- `Morphing Blob`：没有 SVG gooey 滤镜，用软边径向渐变模拟融合；悬停时收拢成软环，
  不变成网页元素形状。
- `Glitch Shift`：上游用 `mix-blend-mode: screen`，这里用加色图元近似。
- `Spotlight` / `Torch Light`：暗罩压在 base 层，`clear_circle` 挖洞后补柔边；与
  CursorFX 同名效果同一套做法。
- 没有中心标记的效果（Particle Trail、Fire Trail、Wind Stream 等）额外画一个带暗边
  的小点，保证浅色背景上也能找到指针。
