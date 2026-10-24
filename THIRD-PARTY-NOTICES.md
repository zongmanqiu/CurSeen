# 第三方组件许可与说明（THIRD-PARTY-NOTICES.md）

> 随发布包分发。CurSeen 自身以 **GNU AGPL-3.0** 授权（见 `Cargo.toml` 的 `license` 字段）。
> 本文件列出直接依赖的第三方组件、它们的许可证、**从哪里取到**，以及 CurSeen 对其中
> 一部分所做的 **本地修改**——AGPL-3.0 要求分发时保留这些声明。

完整、权威的依赖树与版本见 `Cargo.lock`；本文件只登记**直接依赖**中需要署名 / 有分发
义务 / 被本地修改过的部分。新增或移除依赖时必须同步本文件（见 `DOCS_MANIFEST.md`）。

## 关于许可证栏里的「OR」

「MIT OR Apache-2.0」这类写法**不是含糊，而是精确**——它是 **SPDX 许可表达式**
（SPDX License List，ISO/IEC 5962 标准），与上游 crate 在 `Cargo.toml` / crates.io 里
声明的 `license` 字段**逐字一致**，本文件不做任何改写。含义是：该组件的作者同时按
两个许可证授权，**使用者任选其一**满足条款即可。本包对这类组件同时附上两份全文
（见 `licenses/THIRD-PARTY-LICENSES.txt`），所以按任何一个都拿得到需要的文本。
把「OR」自行删改成单一许可证反而是**错误**的——那会谎报上游的授权意图。
CurSeen 自身是单一许可证（AGPL-3.0），不存在这个问题。

## 一、本地修改过的依赖（重点）

| 组件 | 版本 | 许可证 | 位置 | 修改 |
|---|---|---|---|---|
| `windui` | 0.30.0 | MIT OR Apache-2.0 | `.3rdParty/wind-ui-rust-0.30.0`（path 依赖，**不在仓库内**） | 有，逐条登记在 **`patches/wind-ui-rust-0.30.0/PATCHES.md`**；源码里每处用 `★ CurSeen 补丁 0NN` 标注 |

**我们的修改在哪里可以取到**：`patches/wind-ui-rust-0.30.0/` 存放的是**修改后的完整文件**
（不是 diff），与 `PATCHES.md` 的逐条说明一一对应——这就是 AGPL-3.0 / MIT 所要求的
"提供修改"的落点。上游原始归档 + 本仓库的 `patches/`，两者合起来构成可分发的对应源码。

`windui` 自带 `LICENSE-MIT` 与 `LICENSE-APACHE` 两份许可文本，本程序按 MIT OR Apache-2.0
的条款使用该库；两份原文随发布包分发（见第四节）。

### 上游源码的获取方式

唯一的本地 path 依赖不在 git 仓库内，且**不是** crates.io 上的原样版本（本项目用的是打了补丁
的上游源码）。它们的上游地址登记在 `main/build/deps.txt`，构建时自动取回：

| 组件 | 版本 | 上游地址 |
|---|---|---|
| `wind-ui-rust` | 0.30.0 | <https://codeload.github.com/huanfeng/wind-ui-rust/tar.gz/refs/tags/v0.30.0> |

> `nnnoiseless` 曾经也是 path 依赖，2026-09-28 起直接用 crates.io 版（与本仓 vendored
> 版逐字一致，实测确认），故从本表与 `.3rdParty/` 中移除。

## 二、直接依赖（未修改）

| 组件 | 版本 | 许可证 | 用途 |
|---|---|---|---|
| `windows` | 0.62.x | MIT OR Apache-2.0 | Win32 API 绑定（截图 / DXGI 录屏 / Media Foundation 编码 / 剪贴板 / 音频） |
| `mlua`（Lua 5.4, vendored） | 0.12.x | MIT | 鼠标拖尾 / 点击效果等特效脚本引擎 |
| Lua | 5.4 | MIT | 由 `mlua` 的 `vendored` feature 静态编入 |
| `nnnoiseless`（RNNoise 的纯 Rust 移植） | 0.5.2 | BSD-3-Clause | 麦克风降噪（crates.io，`default-features = false`；2026-09-28 前 vendor 于 `.3rdParty/`，未修改过） |
| `tiny-skia`（经 `windui` 引入） | 0.12.x | BSD-3-Clause | CPU 软渲染光栅化（★ 2026-09-28 实测 registry `Cargo.toml` 勘误：此前误写为 MIT OR Apache-2.0；其上游 Skia 祖源是 zlib 风格，但本 crate 自身按 BSD-3-Clause 授权） |
| `serde` / `serde_json` | 1.0.x | MIT OR Apache-2.0 | 配置持久化 |
| `log` / `env_logger` | 0.4 / 0.11 | MIT OR Apache-2.0 | 日志 |
| `png` | 0.18.x | MIT OR Apache-2.0 | 图标产物编码；关于页二维码、悬浮标图片解码 |
| `resvg` / `usvg`（**optional**，仅 `icon-gen` feature） | 0.48.x | Apache-2.0 OR MIT | **构建期**把 SVG 图标 / 二维码源光栅化成位图产物（`examples/gen_icon.rs`）。默认不编译，**既不进 exe 也不随包分发**——产物位图是它的输出，不受其许可约束 |
| `gif` | 0.13.x | MIT OR Apache-2.0 | 悬浮标「图片」类型的 GIF 动画帧解码 |
| `ureq`（native-tls + gzip） | 3.x | MIT OR Apache-2.0 | 更新检查与下载 |
| `native-tls`（Windows 侧落到 `schannel`） | 0.2.x | MIT OR Apache-2.0 | `ureq` 的 TLS 后端；加解密与信任链实际由**系统** Schannel 完成（2026-10-08 起取代 `rustls` + `ring`，理由见 `Cargo.toml` 注释） |
| `chrono` | 0.4.x | MIT OR Apache-2.0 | 时间戳（日志 / 产物命名） |
| `rand` | 0.8.x | MIT OR Apache-2.0 | 特效随机量 |

> 版本号以 `Cargo.lock` 为准；上表记录的是本文件最后更新时的取值。
>
> 上表只列 `curseen-ui` 在 `Cargo.lock` 里的**直接**依赖。`Lua`（mlua vendored）、
> `tiny-skia` / `toml`（经 windui）、`easyfft` / `once_cell`（经 nnnoiseless）等是传递
> 依赖，也随二进制分发许可义务，这里一并列出备查。
>
> ★ **`toml` 虽是传递依赖，但确实编进了产物**（`windui` 的正常依赖，用来读主题配置，
> 不是构建期工具），因此同样有署名义务——**别因为"不是直接依赖"就把它删掉**。
>
> ★★ **许可全文随包分发**：上表是**索引**（组件 / 版本 / 许可 / 上游），正文在
> `licenses/THIRD-PARTY-LICENSES.txt` —— 由 `build/gen_third_party_licenses.py` 从
> `Cargo.lock` + cargo registry 缓存自动收集（部署时自动跑，见 `gen_licenses.py`）。
> 为什么不能只写索引：MIT / Apache-2.0 / BSD-3-Clause 都要求**许可全文随副本一起分发**，
> "原文在 cargo registry 缓存里、随 Rust 工具链分发"对没有 Rust 工具链的接收者不成立。
> 范围 = 直接依赖 + 本文件点名的传递依赖；权威全集是随包的 `Cargo.lock`。

## 二b、随包分发的素材（非代码依赖）

| 素材 | 许可证 | 来源 |
|---|---|---|
| `assets/Sounds/*.wav`（出厂点击 / 悬停 / 开关音效，14 个文件） | CC0 1.0 | [Kenney UI Audio](https://github.com/Calinou/kenney-ui-audio)（Kenney 原 Zip 与本包所收版本一致口径，见 `assets/Sounds/README.md`） |

> CC0 1.0 不要求署名，但本包仍随附其全文（`licenses/CC0-1.0.txt`）与来源声明——
> 便于接收者核验素材出处。素材清单与格式说明见 `assets/Sounds/README.md`。

## 三、系统组件（不随程序分发，运行时调用）

CurSeen 不打包任何第三方 DLL，以下能力全部由 Windows 自带组件提供：

- **DXGI Desktop Duplication** —— 屏幕采集
- **Media Foundation** —— H.264 硬件编码 / 音频采集
- **Win32 / GDI / DWM** —— 窗口、抓屏、窗口外框探测
- **MCI** —— 非 WAV 音频播放（系统解码器）

## 四、随包分发的许可原文

发布目录 `build/` 里已包含全部许可文本，**整目录拷走即可分发**（无需事后补文件）：

```
build/
├── curseen.exe
├── LICENSE                      ← CurSeen 自身：AGPL-3.0 全文
├── licenses/
│   ├── THIRD-PARTY-NOTICES.md       ← 本文件
│   ├── windui-LICENSE-MIT.txt       ← 上游 windui 的 MIT 原文
│   ├── windui-LICENSE-APACHE.txt    ← 上游 windui 的 Apache-2.0 原文
│   ├── CC0-1.0.txt                  ← 出厂声音素材（Kenney UI Audio）的 CC0 原文
│   └── THIRD-PARTY-LICENSES.txt     ← 其余随 exe 分发的组件许可全文（自动生成）
├── assets/
└── config.json
```

> 文件名带包名前缀，因为上游的许可文件名并不统一（windui 的两份分别叫
> `LICENSE-MIT` / `LICENSE-APACHE`）。

## 五、分发义务提示

- 分发 CurSeen 的二进制时，请连同 `build/` 里的 `LICENSE`、`THIRD-PARTY-NOTICES.md`
  与 `licenses/` 一起分发（它们已放在同一目录，整目录拷贝即可）。
- 对 `windui` 的本地修改必须可被获取：`patches/wind-ui-rust-0.30.0/PATCHES.md`
  与 `patches/wind-ui-rust-0.30.0/`（修改后完整文件）即为此而存在。
- 本程序为单 exe + `assets/` 的便携形态，配置、日志、运行期产物都留在程序目录。
