# 声音素材（共用）

> ★ 2026-09-25 由 `ClickSound/` 改名：本目录是**所有发声功能共用**的出厂音源——
> 鼠标点击声音、键盘声音、拖尾音效都默认从这里取；用户也可以在各自面板里
> 浏览选别的文件夹（浏览即引用，程序目录内的记便携相对路径）。

把**音频文件**丢进本目录即可，**不需要改代码、不需要重新编译**：
面板每次打开都会重扫本目录，文件名（去掉扩展名）就是面板下拉里的选项。

## 支持哪些格式

只收 **Windows 原生就能播**的格式——本程序**不捆绑、也不需要任何第三方解码库**：

| 格式 | 谁来解码 | 说明 |
|---|---|---|
| `.wav` / `.wave` | 本程序自己解析 | **推荐**。音量按 10% 档位对样本定标，最精确 |
| `.mp3` `.wma` `.m4a` `.aac` `.aif` `.aiff` `.mid` `.midi` | Windows（MCI） | 系统自带解码器，开箱可用；音量经 `setaudio` 逐声音设定 |

**其它格式不会被读取**（`.flac` `.ogg` `.opus` `.wv` `.ape` `.caf` `.amr` …）：
Windows 多半没有它们的解码器，与其在下拉里放一个"选了也没声"的死选项，不如根本不列出来。
需要的话先转码，例如：

```
ffmpeg -i in.wv out.wav
```

> 想让某个格式出现在下拉里：把扩展名加进 `main/src/platform/audio.rs` 的
> `SUPPORTED_EXT`，**并确认本机确实能播**（`mciSendStringW` 能 open 得开）。

## 文件名与配置

- 配置里记的是**文件名（不含扩展名）**，即 `build/config.json` 里
  **顶层** `sound_bindings.<键>.file`（`{"左键": {"file": "click1", …}}`）。
  ★ 不是 `trail.sound_bindings`——那是 1.0 旧模型的残留路径，新模型一律读写顶层那张表
  （旧字段只作为迁移源保留）。
- 因此把 `click1.wav` 换成 `click1.mp3`（同名不同格式）**不会**让配置失效。
- 同名不同格式同时存在时只取优先级最高的一个（顺序见 `platform/audio.rs` 的
  `SUPPORTED_EXT`，`wav` 优先）。

## 运行期位置

本目录是**运行时依赖**：程序读的是 **exe 同级**的 `assets/Sounds/`
（发布包里的 `build/assets/Sounds/`）。

源码树里的这份 `main/assets/Sounds/` 是素材源，用
`python main/build/sync_assets.py` 单向同步过去（只新增/覆盖，**不做删除镜像**，
所以你在 `build/` 里自己放的文件不会被抹掉）。

## 版权

`click*.wav` / `mouseclick1.wav` / `mouserelease1.wav` / `rollover*.wav` /
`switch1*.wav` 来自 [Kenney UI Audio](https://github.com/Calinou/kenney-ui-audio)，
CC0 1.0——许可声明与 CC0 全文随包分发（见 `THIRD-PARTY-NOTICES.md` 的
「随包分发的素材」一节与 `licenses/CC0-1.0.txt`）。

> ★ 目录即来源：这里每个音频文件（不论代码是否直引其名）都是面板下拉里的一个可选项。
> 1.0 的 `mapping.txt`（内部名→显示名对照）已随大扫除删除——2.0 起显示名直接取文件名。
