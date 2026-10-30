# CursorFX（CurSeen Lua 扩展包）

**参考与致谢：** [CursorFX](https://github.com/devkancheti4-design/cursorfx)（MIT）。

本目录是 CurSeen 对 CursorFX 效果的**独立 Lua 重实现**，用于学习和扩展 CurSeen 的鼠标特效，不是 CursorFX 的官方移植，也没有复制上游的源代码、图片、音频或 GIF。上游 CursorFX 的版权归其作者所有；这里提到它，是为了致谢与说明效果来源。

## 内容

共 42 个效果：

- 17 个光标模型（含 4 把枪、F1 赛车）
- 9 个拖尾
- 16 个点击效果

## 使用

`assets/lua/scripts/CursorFX/` 是**一个包**：目录本身在「脚本」页显示为**一张卡**，目录内平铺的 `.lua` 是卡片标题栏下拉的选项。选择某个子脚本后，下方参数区会切换成该脚本的 `schema()`。

- `pack.json`：包名、版本、许可证、默认项、下拉顺序。
- 子脚本可以单独复制到 `assets/lua/MouseTrail/`、`HoverDot/`、`ClickFx/`，由目标目录决定触发方式。
- 同一时间只运行下拉中选中的一个子脚本。

## 已知简化

- `animals` 不做 emoji 像素采样，改为程序化轮廓粒子。
- `waves` 不做页面 `backdrop-filter` 扭曲，使用源码回退的同心圆环。
- 文字/emoji 目前不做旋转；`emojirain` / `snow` 用正立字符近似，`petals` 用 20 顶点花瓣多边形替代 🌸 贴图。
- 原库的音频合成未移植；需要声音时请用 CurSeen 的 `play_sound` 或点击音效功能。
