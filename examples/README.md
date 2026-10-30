# 离屏验收探针（`examples/`）

本目录存放 **Rust 编写的离屏渲染验收探针**：绕开窗口 / 输入链路（UIPI 会静默吞掉跨完整性
级别的合成输入），直接驱动真实控制器产出 `DrawList`，在进程内做**像素级判据**。
只在开发期使用，**不进发布产物**。（同类真机脚本用 Python 写，一律放根 `temp/`。）

## 为什么放在 `examples/`

这是 Cargo 的惯例目录，且 `Cargo.toml` 里用 `[[example]]` 显式登记了每一个，因此：

```text
cargo run --example shape_probe       # 形状渲染像素级判据（退出码 0 = 全部通过）
cargo run --example text_in_box_probe # 虚拟键盘图例：居中 + 放不下就缩字号
cargo run --example render_probe      # 标注 / 白板离屏渲染，产物写到 temp/render_probe/
cargo run --example gen_update_bat -- <程序目录> <zip 路径> <exe 文件名>  # 生成更新脚本
```

## 文件

- `shape_probe.rs`：箭头"一体成型"与序号"圆内居中"的像素判据（被 `src/modules/drawing/*`
  多处注释引用为验收标准）。
- `text_in_box_probe.rs`：`DrawCmd::TextInBox` 的像素判据 —— 图例墨迹是否在矩形里
  **水平且垂直**居中（容差 ±1.5px），以及**放不下时是否缩了字号**（对照组用不缩放的
  `TextCentered` 证明"这段文字本来放不下"）。实测：居中偏移 `(+0.10, -0.10)px`，
  而模块层自己凑坐标的旧算法是 `(-6.90, +0.90)px` —— 用户报的"Ctrl 靠左"就是这 6.9px。
  产物写到 `temp/probe_out/`。
- `render_probe.rs`：标注 / 白板模式的离屏渲染验收，绕过窗口与输入链路，直接驱动真实
  `DrawingController` 产出 `DrawList` 再经生产翻译层渲染，产物写到 `temp/render_probe/`。
- `cursorx_preview.rs`：CursorX 扩展包 24 个脚本的离屏预览，各跑 90 帧后渲染到合成桌面，
  产物写到 `temp/cursorx_preview/`（单图 + `contact.png` 联系表）。`Difference Blend` /
  `Fluid Glass` 在探针里走静态回退分支（没有覆盖层窗口可采样）。
- `mfcmouse_preview.rs`：MFCMouseEffect 扩展包 25 个脚本的离屏预览，按通道触发
  （Click=抬起 / Trail=移动 / Scroll=滚轮 / Hold=按住 500ms / Hover=静止 1.6s /
  Cursor=常驻），产物写到 `temp/mfcmouse_preview/`。
- `gen_update_bat.rs`：复用线上同一份 `bat_script`，把更新脚本写到任意沙箱目录，便于手工 /
  自动化验证更新流程。
- `crash_probe.rs`：**故意触发一次访问冲突**，验证 `platform/crash.rs` 真的落盘。
  会崩掉进程（预期行为），勿放进 `cargo test`。
- `frame_cost_probe.rs`：把一帧拆成【清屏 / 推进 / 光栅 / 换序】四段计时
  （各开/关组合：拖尾+聚光灯、只聚光灯、只小圆点悬浮标、全关），定位卡顿主因。
  只读、不开窗、不改系统状态，手动 `cargo run --release --example frame_cost_probe` 跑。
- `spotlight_partial_probe.rs`：聚光灯**整窗 vs 增量**路径的一帧成本对比（三档分辨率）。
  走生产同一套 `detect_spotlight_partial`，增量帧跳过整窗清屏、R/B 换序只做脏区。
  只读、不开窗、不改系统状态。
- `window_title_probe.rs`：**动态标题栏**的真机判据（裁剪窗那句"文档名 - 软件名"）。
  建一个绑了 `Signal<String>` 标题的子窗，每 400ms 改一次标题，并用系统的 `FindWindowW`
  （只认完整标题，正是按标题关窗 / 重画用的同一个 API）验证能按**新**标题命中；
  命中退 0，落空退 1。会开两个窗、约 1.5s 自动退出。
- `mp4_probe.rs`：MP4 预览解码与裁剪的端到端探针（时长 / 逐时刻解帧 / 裁剪产物自检）。
  跑法见文件头；`MP4_PROBE_DUMP_RAW=<目录>` 落原始 RGBA 供 PIL 转 PNG 人眼核对行序与颜色。
