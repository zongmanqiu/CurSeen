# CurSeen 可寻：键鼠所至 · 屏幕所书 · 不止所见

> 小而美的教学演示辅助工具 (Win10 2004+)

**[⬇️ 下载 2.1.0 (Gitee)](https://gitee.com/qiuzongman/curseen/releases/download/2.1.0/CurSeen_2.1.0.zip)**
 ｜ **[⬇️ 下载 2.1.0 (GitHub)](https://github.com/zongmanqiu/CurSeen/releases/download/2.1.0/CurSeen_2.1.0.zip)**
 ｜ [Gitee主仓](https://gitee.com/qiuzongman/curseen)
 ｜ [GitHub镜像](https://github.com/zongmanqiu/CurSeen)
 ｜ [B站视频演示](https://www.bilibili.com/video/BV18pp46QEkk/)

## 功能一览
- **鼠标轨迹**：鼠标拖尾、悬浮标（跟随光标，含聚光灯 / 激光笔等脚本）、点击波纹与特效。
- **键盘呈现**：点击声音、击键回显（屏幕显示你按下的键）、虚拟键盘（主播向布局 + 高亮注入）。
- **屏幕标注**：透明 / 白板画笔，支持撤销 / 重做 / 清空；截图后可进编辑态标注。
- **截图与贴图**：截图进剪贴板或钉到桌面，贴图可移动 / 缩放 / 另存为（`Ctrl+S` 存当前画面）。
- **录制**：MP4 视频录制（分辨率 / 帧率 / 硬件加速回退）+ GIF 录制（选区 / 帧间差量 / 裁剪另存）。
- **辅助**：放大镜（画中画 / 全屏，滚轮调倍率）、计时器（正 / 倒计时，可调锚点与字号）、
  便签、剪贴板历史（粘贴板回顾）。
- **可扩展**：鼠标 / 点击 / 悬浮标 / 击键音效 / 通用扩展几乎都是 **Lua 脚本**，丢文件即生效、
  不重编译；也可在「脚本」页顶部「官方资源下载」直接安装官方包（Gitee / GitHub 双源）。
  脚本 API 见 `assets/lua/API.md`。
- **轻量**：CPU 软渲染、无外部运行时依赖，单可执行文件即装即用；空闲零功耗、关闭即释放资源。

> 更完整的隐藏快捷键与技巧见随包分发的 **`assets/manual/隐藏快捷键手册.html`**（安装目录 `assets/manual/` 内）。

## 快速开始

### 环境要求

- Windows 10 2004 及以上——画中画 / 全屏放大的防自我嵌套需要 2004 才有的系统 API，
  更低版本上放大窗画面会自我嵌套（套娃）
- Rust 工具链（≥ 1.94，edition 2021）
- 网络连接（首次构建需从 crates.io 拉取依赖）

### 构建

**一键（推荐）**：

```bat
main\build\full.bat
```

它按顺序跑六步，任一步失败即停：

| 步骤 | 脚本 | 干什么 |
|---|---|---|
| 1 | `build\1_check_env.bat` | 只读体检：cargo / rustc host 是否 MSVC、python、git、`.3rdParty/` |
| 2 | `build\2_download.bat` | 按 `build\deps.txt` 下载第三方源码到 `.3rdParty/`（已有则跳过） |
| 3 | `build\3_patch.bat` | 把 `patches/<库>-<版本>/` 单向覆盖到 `.3rdParty/`（重放我们的补丁） |
| 4 | `build\4_build.bat` | `cargo build --release` |
| 5 | `build\5_test.bat` | **真门禁**：集成测试 + 单元测试，失败即停（不会部署） |
| 6 | `build\6_deploy.bat` | 拷 exe 到 `build/` 并 MD5 对账 + 同步 `assets/` + 同步许可文件 |

> **全新克隆也能直接跑**：`.3rdParty/` 在仓库之外、克隆出来是空的，②③ 会负责
> 下载上游源码并重放补丁，所以没有"先手工准备依赖"这一步。

按需单跑：`build\4_build.bat`（只改了代码）、`build\5_test.bat`（提交前）、
`build\6_deploy.bat`（只想刷新 `build\`）。

等价的手工命令：

```bat
cd main
cargo build --release                 REM 产物目录由 .cargo/config.toml 指定为 ../temp/target
python build/deploy.py                REM 组装到 <项目根>/build/（拷 exe + 同步 assets/ + 同步许可）
```

> **目录易混**：`main\build\` 是**构建脚本**（源码，要上传）；`<项目根>\build\` 是
> **产物**（exe + 运行期数据）。中间产物在 `temp/target/`，不在源码树里。

### 运行测试

```bat
cargo test --release -- --test-threads=1    REM 全量（单线程；多线程在部分机器会被杀软持锁卡死）
```

> 集成测试（`tests/`）才算真门禁——`--lib` 会跳过 `tests/docs_consistency.rs`（文档一致性门禁）。

### 打包发布

```bat
main\build\6_deploy.bat               REM 或 cargo build --release + python build/deploy.py
```

产物在 `<项目根>/build/`：`curseen.exe` + `assets/` + `LICENSE` + `THIRD-PARTY-NOTICES.md`
+ `licenses/`，**该目录本身即可直接分发**。版本号同步与发行步骤见 `docs/RELEASE_CHECKLIST.md`。

## 目录结构

```
main/                              ← 上传单位（源码 + 现行文档 + 构建脚本）
                                      .3rdParty/ temp/ build/ 都在它之外
├── Cargo.toml                     包定义（bin = curseen；[[example]]/[[test]] 登记探针）
├── Cargo.lock                     依赖锁定
├── build.rs                       注入构建信息（git 短哈希 → "关于"页/日志；图标资源经 rc.exe 编入）
├── README.md                      本文档
├── AGENTS.md                      项目规则（AI 工具须遵守的硬约束）
├── 目录结构说明.md                每个目录 / 文件的职责与分层依赖（接手必读）
├── DOCS_MANIFEST.md               文档清单（改代码后逐项核对，docs_consistency 门禁）
├── 开发指南.md                    开发/发版流程与提交前检查清单（修改流程唯一正本）
├── HISTORY.md                     版本历史（要点，顶部追加）
├── THIRD-PARTY-NOTICES.md         第三方许可与本地修改说明
├── LICENSE                        AGPL-3.0 官方全文
├── .gitignore / .gitattributes    仓库卫生（*.bat 强制 CRLF）
├── src/                           源码（分层：core←platform←scripting←modules←app←ui←runtime）
│   ├── main.rs                    程序入口
│   ├── lib.rs                     库入口（装配、单实例闸门）
│   ├── app/                       应用层：全局热键、托盘、UiMsg 消息
│   ├── core/                      纯逻辑：配置 / 几何 / 颜色 / 图像 / 绘制指令 / 文本度量
│   ├── modules/                   功能模块：拖尾 / 按键显示 / 击键音效 / 屏幕书写 / 虚拟键盘 / 放大镜 / 便签 / 剪贴板历史
│   ├── platform/                  Win32 封装：截图 / 录屏 / 显示器 / 窗口 / 键鼠钩子
│   ├── runtime/                   运行时：覆盖层窗口 / 帧调度 / 渲染管线 / 贴图
│   ├── scripting/                 Lua 绑定（特效脚本引擎）
│   ├── ui/                        控制面板 UI：页面 / 模型 / 卡片组件
│   └── update/                    自动更新：版本检查 / 下载 / 安装
├── assets/                        资源（目录即来源）：lua/（唯一脚本根）/ Sounds（共用声音素材）/ docs（商店与更新清单缓存）/ …
├── tests/                         集成测试门禁（docs_consistency / config_pipeline_e2e）
│   ├── probes/                    真机探针（11 条，需硬件或真实窗口，[[test]] 登记）
│   └── fixtures/                  共享素材（config_v1.json）
├── examples/                      Rust 离屏验收探针（cargo run --example，不进发布产物）
├── build/                         构建脚本（编号 bat）+ 构建辅助 python —— 是源码，要上传
├── patches/                       第三方补丁登记 + 补丁后完整文件（wind-ui-rust-0.30.0/）
└── docs/                          现行文档：DESIGN / PLATFORM_NOTES / RELEASE_CHECKLIST / UI_COMPONENTS / 踩坑记录（分册）
```

> **仓库之外**（项目根）：`.3rdParty/`（两个 path 依赖，由 ②③ 从 `build\deps.txt` 取回并打补丁，
> **不在仓库里**）、`temp/target/`（编译中间）、`build/`（产物：exe + 许可文件 + 运行期数据）、
> `temp/`（一次性探针 / 草稿，不入库）。

## 技术栈

| 组件 | 说明 |
|---|---|
| Rust（edition 2021） | 全部第一方源码 |
| windui 0.30.0（含 CurSeen 补丁） | UI 框架 + 分层覆盖窗；CPU 软渲染（tiny-skia 后端） |
| mlua 0.12（Lua 5.4, vendored） | 鼠标拖尾 / 点击效果等特效脚本引擎 |
| windows 0.62 | Win32 API 绑定（截图 / DXGI 录屏 / MediaFoundation 编码 / 剪贴板 / 音频） |
| `nnnoiseless` 0.5.2（path 依赖） | 麦克风降噪（BSD-3，纯 Rust 移植，无 C 绑定） |
| serde / serde_json | 配置持久化 |
| png / gif | 图标、关于页二维码、悬浮标 GIF 解码 |
| ureq | 更新检查与下载（rustls + gzip） |
| 构建 | `opt-level = "z"` 体积优先；仅渲染内核（tiny-skia / windui）提至 3 |

## 架构总览

- **分层依赖（逆向依赖一律视为缺陷）**：

  ```
  core ← platform ← scripting ← modules ┐
    ↑                                   ├ app ← ui ← runtime
    └────────── update ─────────────────┘
  ```

- **配置即面板**：控制面板把下拉 / 滑块解析回配置；出厂默认值已写为"解析结果"，避免首帧
  `apply_to` 误报"配置有改动"。配置落在程序目录 `config.json`（便携化：删目录 = 卸载干净）。
- **目录即来源（扩展不重编译）**：Lua 脚本统一在 `assets/lua/`（唯一脚本根，递归：
  `lua/MouseTrail/`、`lua/HoverDot/`、`lua/ClickFx/`、`lua/KeySound/`、`lua/scripts/`
  （通用扩展与在线包；v2 包 = `scripts/<id>/pack.json` + 平铺脚本 + `info/`）），
  声音素材在 `assets/Sounds/`（点击声音/键盘声音/拖尾音效共用，用户也可浏览选别的文件夹）
  —— 用户丢文件即出现在 UI。
- **覆盖层架构**：`core::Backdrop::paint_into` 是底图唯一入口（放 core 不放 runtime，因
  `modules/drawing` 不得依赖 runtime）；退出策略参数化（`ExitPolicy` + 区域类型）。
- **单实例 + 提权**：面板提权运行以接收提权窗口前台输入；UIPI 会静默吞掉跨完整性级别的合成
  输入，故离屏验收探针（`examples/`，见其 `README.md`）绕过窗口 / 输入链路，直接驱动真实
  控制器做像素级判据。

详见《目录结构说明.md》与 `docs/DESIGN.md`。

## 文档地图

| 想找什么 | 去哪 |
|---|---|
| 怎么构建 / 常见坑 | `build/build_readme.md` |
| 该动哪个文件 / 目录职责 / 分层规则 | `目录结构说明.md` |
| 硬约束与环境坑（AI 与人都看） | `AGENTS.md` |
| 改完代码要核对什么 / 修改流程步骤 | `开发指南.md`（唯一正本）；`DOCS_MANIFEST.md`（文档清单） |
| 功能取舍 / 架构设计 | `docs/DESIGN.md` |
| Win32 实测结论 | `docs/PLATFORM_NOTES.md` |
| 发版流程 | `docs/RELEASE_CHECKLIST.md` |
| 面板组件库 / UI·Lua API 参考 | `docs/UI_COMPONENTS.md` |
| 写 Lua 脚本（脚本作者向，随程序分发） | `assets/lua/Lua脚本手册.html`（`API.md` 为完整 API 参考） |
| 第三方改动 | `patches/wind-ui-rust-0.30.0/PATCHES.md` |
| 版本历史 | `HISTORY.md` |
| 踩坑与教训（现象 / 根因 / 判据） | `docs/踩坑记录*.md`（分册，按 §N 编号查） |

## License

Copyright © 2026 邱宗满 <qiuzongman@foxmail.com>

CurSeen 以 [GNU AGPL-3.0](LICENSE) 授权（见 `Cargo.toml` 的 `license` 字段）。第三方组件的许可证、下载来源与 CurSeen 对它们的修改说明见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)；补丁明细见 `patches/wind-ui-rust-0.30.0/PATCHES.md`。
