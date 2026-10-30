# MFCMouseEffect（CurSeen Lua 扩展包）

**参考与致谢：** [MFCMouseEffect](https://github.com/sqmw/MFCMouseEffect)（MIT）。

本目录是 CurSeen 对 MFCMouseEffect 效果的**独立 Lua 重实现**，用于学习和扩展 CurSeen 的鼠标特效，不是 MFCMouseEffect 的官方移植，也没有复制上游的源代码、图片、音频、3D 模型或其它素材。上游 MFCMouseEffect 的版权归其作者所有；这里提到它，是为了致谢与说明效果来源。

## 内容

共 25 个效果（与上游默认构建一致）：

- **Click（3）**：Ripple、Star、Text
- **Trail（6）**：Meteor Shower、Neon Streamer、Cyber Electric、Sci-Fi Tubes、Particle、Line
- **Scroll（3）**：Arrow、3D Helix、Stardust Stream
- **Hold（7）**：Energy Charge、Lightning、Hex Shield、Tech Ring (3D)、Hologram (3D)、Neon HUD (3D)、FluxField HUD CPU
- **Hover（2）**：Glow、Helix Suspension
- **光标装饰（4）**：Focus Ring、Signal Ring、Soft Orb、Halo Orb

同一时间只运行下拉中选中的一个效果（CurSeen 包卡模型）；上游是六个通道各自独立并存。

## 每个效果怎么触发、看什么

> 先确认包卡标题左侧开关是开的，并把鼠标移到**桌面空白处**再触发（设置面板会挡住一部分视线）。
> Click 在**抬起**时触发；按住 ≥260ms 视为长按，不触发本次 Click。

| 脚本 | 作用 / 视觉 | 触发 |
|---|---|---|
| Ripple | 点击点扩散的蓝色水波纹（主环 + 3 层柔光） | 单击（<260ms） |
| Star | 十角星扩张并淡出 | 单击 |
| Text | 随机中文词从点击点上浮、摆动、缩放 | 单击 |
| Meteor Shower | 流星锥形尾迹 + 火花 + 头部光晕 | 移动鼠标 |
| Neon Streamer | 两层描边的霓虹流光，头重尾轻 | 移动鼠标 |
| Cyber Electric | 抖动/分叉的赛博电弧 | 移动鼠标 |
| Sci-Fi Tubes | 三条滞后跟随的发光管链 | 移动鼠标 |
| Particle | 按移动距离喷出的彩色粒子 | 移动鼠标 |
| Line | 按年龄淡出的普通折线 | 移动鼠标 |
| Arrow | 沿滚轮方向的 3 层 V 形箭头 | 滚动滚轮 |
| 3D Helix | 沿滚轮方向展开的伪 3D 双螺旋 | 滚动滚轮 |
| Stardust Stream | 沿滚轮方向喷出的星尘粒子 | 滚动滚轮 |
| Energy Charge | 按住蓄力的进度环 + 头部亮点 | **按住 ≥260ms**（约 0.5s 可见） |
| Lightning | 球形闪电核心 + 24 条径向电弧 | 按住 ≥260ms |
| Hex Shield | 三层反向旋转的六边形护盾 | 按住 ≥260ms |
| Tech Ring (3D) | 3 个旋转/倾斜的陀螺环 + 球面粒子 | 按住 ≥260ms |
| Hologram (3D) | 50° 倾斜的全息环 + 上升粒子 | 按住 ≥260ms |
| Neon HUD (3D) | 玻璃环 + 进度弧 + 胶囊头 + 百分比 | 按住 ≥260ms |
| FluxField HUD CPU | 6 层脉动环 + 分段弧 + 84 个轨道粒子 | 按住 ≥260ms |
| Glow | 四向准星刻度 + 绕行圆点的呼吸灯 | **静止 1500ms** |
| Helix Suspension | 三条 20 节点光链绕光标旋转 | 静止 1500ms |
| Focus Ring | 软光 + 外/内双环的光标装饰 | 选中即显示（移动鼠标看指针） |
| Signal Ring | 大柔光 + 双环 + 白核心 | 选中即显示 |
| Soft Orb | 宽柔光 + 柔光球 + 白核心 | 选中即显示 |
| Halo Orb | 宽光晕 + 中球 + 细环 + 白核心 | 选中即显示 |

**看不到时的排查顺序**：包卡开关 → 是否用了正确的触发（Click 单击/Scroll 滚轮/Hold 按住/
Hover 静止 1.5s）→ 鼠标是否停在设置面板上（移到桌面再看）→ 效果颜色/尺寸是否被滑条调到
过低。滚动特效默认时长比上游稍长（380–400ms），Stardust 的尺寸/数量/粒子寿命也放大了
（默认 400ms 时粒子 300–550ms，且寿命随「时长」滑条同比缩放 + 起手闪光），否则上游
95–215ms 的粒子在 400ms 的 burst 里撑不到后半段；可在参数里调回上游的时长 / 1.0 倍。

## 触发语义

| 通道 | 触发 |
|---|---|
| Click | 鼠标**抬起**触发；按住 ≥260ms 视为长按，不触发本次点击（与上游一致） |
| Trail | 鼠标移动时累积轨迹；`wall_ms` 计时老化 |
| Scroll | 滚轮事件；正/负增量决定方向，横向滚轮也支持 |
| Hold | 按下 260ms 后显示，进度按按住时长推进；抬起淡出 |
| Hover | 指针**静止 1500ms** 后显示；移动/点击立即隐藏（不是“悬停在元素上”） |
| 光标装饰 | 始终跟随指针；`hide_cursor = true` 只隐藏箭头，保留 I 形/手形/缩放提示 |

`on_wheel(delta, horizontal, x, y, params)` 是本包新增的基座能力：只有当前脚本声明了
它，鼠标钩子才转发普通滚轮（否则零开销）。

## 已知简化

- `Neon HUD (3D)`：保留玻璃环、进度弧、胶囊头、百分比与少量触须；去掉的除最密编织纹理外，还有内扫描线、水晶种与分支触须火花，以及出现时的 scale-in。
- `FluxField HUD CPU`：保留 6 层环、分段弧、轨道粒子与中心光晕；分段弧 3 带共 30 段（上游 4 带共 152 段，约 1/5 且缺最外带），轨道粒子 84 个与上游相同未减。
- `Cyber Electric` 的逐帧抖动用 Lua 随机数实时生成，不保证与 C++ 每一帧同形。
- 粒子上限按 CurSeen 软渲染约束（≤300）设定，不照搬 GPU 版的 500–30000。
- 上游两个 GPU-only hold 变体在默认构建中本来就会回退/不显示，本包不单列；GPU 磁通场
  的外观由 CPU 版承担。
- 文字类不旋转（渲染层没有旋转文字）；点击文字用正立字符。

## 使用

`assets/lua/scripts/MFCMouseEffect/` 是一张包卡：目录内平铺的 `.lua` 是卡片标题栏下拉的
选项，选中哪一项就只运行哪一项。子脚本也可以单独复制到 `assets/lua/MouseTrail/`、
`HoverDot/`、`ClickFx/`，由目标目录决定触发方式（Scroll/Hold 建议留在包卡里）。
