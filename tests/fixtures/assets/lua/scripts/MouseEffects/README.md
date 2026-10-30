# MouseEffects（CurSeen Lua 扩展包）

**参考与致谢：** [MouseEffects](https://github.com/ltrudu/MouseEffects)（MIT）。

本目录是 CurSeen 对 MouseEffects 效果的**独立 Lua 重实现**，用于学习和扩展 CurSeen 的鼠标特效，不是 MouseEffects 的官方移植，也没有复制上游的源代码、图片、音频、着色器或其它素材。上游 MouseEffects 的版权归其作者所有；这里提到它，是为了致谢与说明效果来源。

## 内容

上游是 DirectX 11 的插件式 GPU 特效应用（45 个插件，可多效果并存）。CurSeen 包卡同一时间只运行一个子效果，因此本包收录**能在光标宿主落地的 33 个**：

- **Click（6）**：Confetti、Crystal Growth、Firework、Ink Blot、Pixel Explosion、Shockwave
- **Trail（6）**：Circuit、Fire Trail、Laser Work、Particle Trail、Pixie Dust、Smoke
- **Cursor（21）**：Aurora、Black Hole、Bubbles、Cherry Blossoms、DNA Helix、Dandelion Seeds、Falling Leaves、Fireflies、Gravity Well、Hearts、Lightning Storm、Nebula、Portal、Procedural Sigil、Sacred Geometries、Snowfall、Spirograph、Spotlight、Starfield Warp、Tesla、Zoom

未纳入的 12 个插件见下方「与上游的差异」。

## 每个效果怎么触发、看什么

> 先确认包卡标题左侧开关是开的，并把鼠标移到**桌面空白处**再触发（设置面板会挡住一部分视线）。
> Click 在**抬起**时触发；按住 ≥260ms 视为长按，不触发本次 Click。

| 脚本 | 作用 / 视觉 | 触发 |
|---|---|---|
| Confetti | 点击爆开的彩色纸片（矩形/圆片/细带） | 单击 |
| Crystal Growth | 从点击点分叉生长的冰晶，枝端有亮核 | 单击 |
| Firework | 默认点击点原位爆（球/环/垂柳/棕榈四样式随机），可选升空火箭模式 | 单击 |
| Ink Blot | 点击处晕开的墨团与卫星墨点，受重力下落、近距互吸融合 | 单击 |
| Pixel Explosion | 点击爆开的复古像素方块 | 单击 |
| Shockwave | 点击扩散的冲击波圆环（可叠 1–4 圈） | 单击 |
| Circuit | 沿鼠标轨迹生长的电路走线，带 90° 拐弯与分叉 | 移动鼠标 |
| Fire Trail | 跟随鼠标的火焰 + 烟 + 余烬 | 移动鼠标 |
| Laser Work | 随移动向四周发射的彩色激光，屏幕边缘反弹、互撞爆炸 | 移动鼠标 |
| Particle Trail | 跟随鼠标的渐变粒子 | 移动鼠标 |
| Pixie Dust | 洒下的闪光仙尘（光点 + 四芒星） | 移动鼠标 |
| Smoke | 跟随鼠标上升的软边烟雾 | 移动鼠标 |
| Aurora | 光标上方的多层流动极光帘幕 | 选中即显示 |
| Black Hole | 事件视界 + 吸积盘 + 可选桌面透镜 | 选中即显示 |
| Bubbles | 从光标升起、可左键戳破的肥皂泡 | 选中即显示 |
| Cherry Blossoms | 围绕光标飘落的樱花 | 选中即显示 |
| DNA Helix | 跟随光标的双螺旋 | 选中即显示 |
| Dandelion Seeds | 围绕光标漂浮的蒲公英种子 | 选中即显示 |
| Falling Leaves | 围绕光标翻落的秋叶 | 选中即显示 |
| Fireflies | 围绕光标明灭游走的萤火虫；点击时从点击点炸开 | 选中即显示 / 单击 |
| Gravity Well | 围绕光标的牛顿引力碎屑井（可坠入/逃逸/屏幕反弹） | 选中即显示 |
| Hearts | 围绕光标上浮的爱心 | 选中即显示 |
| Lightning Storm | 围绕光标周期性劈落的闪电 | 选中即显示 |
| Nebula | 旋转的彩色星云 + 星点 | 选中即显示 |
| Portal | 旋转的传送门 + 螺旋臂 | 选中即显示 |
| Procedural Sigil | 旋转法阵、星形与符文刻痕 | 选中即显示 |
| Sacred Geometries | 生命之花 / 梅塔特隆 / Sri Yantra 三选一 | 选中即显示 |
| Snowfall | 围绕光标飘落的六角雪花 | 选中即显示 |
| Spirograph | 跟随光标的旋转万花尺图案 | 选中即显示 |
| Spotlight | 全屏压暗 + 柔边光孔 + 尘埃 | 选中即显示 |
| Starfield Warp | 从光标向外跃迁的星场 | 选中即显示 |
| Tesla | 围绕光标放电的特斯拉电弧 | 选中即显示 |
| Zoom | 跟随指针的圆形桌面放大镜 | 选中即显示 |

**看不到时的排查顺序**：包卡开关 → 是否用了正确的触发（Click 单击 / Trail 移动 / Cursor 常驻）→
鼠标是否停在设置面板上 → 尺寸/亮度/数量是否被滑条调得过低。Black Hole 与 Zoom 依赖桌面抓屏，
如果系统不允许抓屏（`backdrop_circle` 返回 false），会退回静态外观；此时镜片内容不可见，但
光圈/镜圈仍在。

## 触发语义

| 通道 | 触发 |
|---|---|
| Click | 鼠标**抬起**触发；按住 ≥260ms 视为长按，不触发本次点击 |
| Trail | 鼠标移动时累积；`wall_ms` 计时老化 |
| Cursor | 选中即常驻，指针静止也出画 |

21 个 Cursor 效果都会隐藏系统箭头，并在**真实指针位置**画一个黑外圈 + 白内芯的定位点
（浅色/深色桌面都能看见）；极光、星云、樱花、萤火虫这类随机游走图案也能一眼找回鼠标。

## 与上游的差异

### 未纳入的 12 个插件

它们是全屏滤镜、街机游戏或需要全屏位移采样，不是"跟随光标画图"的鼠标特效。CurSeen 目前只有
`ctx:backdrop_circle`（单圆桌面采样，放大/差分），没有全屏捕获、像素着色器、矩形镜片或游戏
HUD 通道；为它们改整个渲染架构不属于一个鼠标特效包的成本收益。

| 上游插件 | 类别 | 不纳入原因 |
|---|---|---|
| ASCIIZer | VisualFilter | 全屏 ASCII 着色器（6 模式 + CRT） |
| ColorBlindnessNG | VisualFilter | 全屏 CVD LUT 校正 |
| Glitch | VisualFilter | 全屏数字故障着色器 |
| Hologram | VisualFilter | 全屏全息扫描线着色器 |
| Kaleidoscope | VisualFilter | 全屏万花筒镜像 |
| RadialDithering | VisualFilter | 全屏 Bayer 抖动 |
| Retro | VisualFilter | 全屏 CRT/LCD/VHS 滤镜 |
| TileVibration | VisualFilter | 全屏分块采样 + 振动 |
| WaterRipple | VisualFilter | 全屏位移采样（波浪物理） |
| Invaders | Interactive | 街机小游戏（分数/波次/输入） |
| RetroCommand | Interactive | 导弹指挥官小游戏 |
| Retropede | Interactive | 蜈蚣街机小游戏 |

### 保留效果的简化

- **GPU → CPU**：上游普遍用 500–10000 个 GPU 粒子和像素着色器；这里按 CurSeen 软渲染约束
  把粒子压到 60–300、线框压到 ≤400 图元，外观保留主要层次。
- **Firework 样式压缩**：上游 Styles/ 有 19 种爆炸样式（Palm/Peony/Willow 等），这里实现球/环/垂柳/棕榈四种常见形随机取一；升空火箭为可选参数（默认关，上游默认也是原位爆）。
- **Black Hole**：上游全屏引力透镜；这里用一枚可选的 `backdrop_circle` 放大镜 + 纯绘制
  吸积盘/光子环。关掉「桌面透镜」则完全不抓屏。
- **Bubbles**：上游可选屏幕折射；这里保留透明填充、彩边与高光，不做逐泡折射。
- **Shockwave**：上游可选屏幕畸变；这里保留多层光环。
- **Spotlight**：上游用捕获画面做亮度提升；这里用 base 层暗罩 + `clear_circle` 挖孔 +
  尘埃，等价于舞台追光。
- **Zoom**：上游支持圆形/矩形镜片；CurSeen 目前只有圆形桌面采样，本包只做圆形。
- **Confetti / Pixel Explosion**：上游同时支持点击爆发与移动撒落；本包按 Click 通道只做点击。
- **Spirograph**：上游把图案画在鼠标移动轨迹上；这里做成跟随光标的完整旋转图案。
- **游戏/滤镜**：见上表，未实现。

## 参数与性能

- 每个效果 5–9 个参数，优先保留"改变画面最明显"的上游项；上游 94 项的 Tesla、69 项的
  Firework 等压缩为可解释的少数参数。
- 尺寸用像素、时长用秒 / 毫秒（标签标明），数量有 CPU 上限；默认值全部落在滑条的
  `min + n × step` 网格上（由 `all_pack_schemas_have_sane_ranges` 门禁检查）。
- **颜色参数带 alpha（0–255）**：与各效果的 `opacity`、亮度、glow 相乘，默认不透明。
  40 个颜色全部通过"alpha 255 vs 128"的动态对照；颜色 alpha 调低时效果本体变淡，
  但黑白光标定位点始终保留，方便找回鼠标。
- 固定颜色（Confetti / Pixel Explosion / Firework / Laser Work / Pixie Dust）的 **RGB**
  只在关闭"彩虹/随机色"时生效；**alpha 始终作为整体透明度参与合成**。动态审计在
  `rainbow = false` 下验证 RGB，默认彩虹模式下也验证 alpha。
- 单帧指令数大致范围：轻量环境效果 20–120 条，粒子/线框效果 100–300 条；粒子池硬上限
  见各脚本顶部注释。`rate`/`count` 到达池上限后表现为"更快的替换"而不是无限增密，
  这是软渲染的安全阀。

## 使用

`assets/lua/scripts/MouseEffects/` 是一张包卡：目录内平铺的 `.lua` 是卡片标题栏下拉的
选项，选中哪一项就只运行哪一项。子脚本也可以单独复制到 `assets/lua/MouseTrail/`、
`HoverDot/`、`ClickFx/`，由目标目录决定触发方式。
