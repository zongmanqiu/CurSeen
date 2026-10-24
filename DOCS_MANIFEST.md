# DOCS_MANIFEST.md

> 文档清单。列出全部**随代码变化**的文档文件及其更新时机。
> 每次改动完成后**逐项核对**本清单。
> `tests/docs_consistency.rs` 会验证清单里的文件真实存在且结构描述成立——改文档后必须保持全绿。
>
> **两分法**：有长期价值的文档（含方案 / 说明 / 踩坑记录）放 `main/docs/` **入库**并在本清单登记
> （`test_repo_holds_no_process_documents` 会核对登记）；一次性草稿 / 实测转储放根 `temp/`，
> 不入库。

## 一、必须随代码同步的文档（受门禁约束）

| # | 文件 | 何时必须更新 | 由谁验证 |
|---|---|---|---|
| 1 | `AGENTS.md` | 项目规则变化；发现新的环境坑 / 源码陷阱；新增硬约束 | `test_docs_checks_the_manifest_files_exist` |
| 2 | `目录结构说明.md` | 增删源码文件或目录；改变某个文件的职责；分层变动 | `test_structure_doc_lists_every_source_module` |
| 3 | `DOCS_MANIFEST.md`（本文件） | 清单本身变化时 | `test_docs_checks_the_manifest_files_exist` |
| 4 | `patches/wind-ui-rust-0.30.0/PATCHES.md` | **任何**对 `.3rdParty/wind-ui-rust-0.30.0` 的修改（编号递增）。0.19 / 0.20 时代的历史章节原样并入本表，只增不改 | `test_patch_record_has_a_section_for_every_windui_patch` |
| 5 | `README.md` | 项目总说明：顶层目录树 / 技术栈 / 构建 / 架构描述变化 | （`Cargo.toml` 的 `readme` 字段依赖其存在） |
| 6 | `HISTORY.md` | 发新版本（**顶部**按主题追加**要点**，旧内容不改）；踩到的坑写进 `docs/踩坑记录*.md`。规范见《开发指南.md》§五 | `test_docs_checks_the_manifest_files_exist` + `test_controlled_docs_stay_concise` |
| 7 | `THIRD-PARTY-NOTICES.md` | 新增/移除/更换任何第三方依赖，或依赖版本变化 | `test_docs_checks_the_manifest_files_exist` |
| 8 | `docs/DESIGN.md` | 功能取舍、分层架构、防打架约定、UI 分类、路线图变化（含视觉令牌 / 卡片结构结论）；体积策略变化（§10：方法 / 实测 / 终点判据） | `test_docs_checks_the_manifest_files_exist` |
| 8b | `docs/DESIGN_ADOPTION.md` | **外部规范 / 通用建议的裁决结果**：采纳了哪条、落到哪个文件、不采纳的理由、已知缺口。读了新规范先查这里，已裁决的条目不重新讨论 | `test_docs_checks_the_manifest_files_exist` |
| 9 | `docs/PLATFORM_NOTES.md` | **新测出来的** Win32 行为（凭印象下结论必须先写探针测过） | （门禁未强制；但凡拿 Win32 当依据先查这里） |
| 10 | `docs/RELEASE_CHECKLIST.md` | 发版流程 / 门禁步骤 / 自动更新地址规则变化 | （发版时逐条打勾） |
| 11 | `build/build_readme.md` | 构建流程、脚本职责、环境变量、常见坑变化 | `test_docs_checks_the_manifest_files_exist` |
| 12 | `patches/README.md` | 补丁目录规则变化 | `test_docs_checks_the_manifest_files_exist` |
| 13 | `docs/踩坑记录.md` | **踩到坑 / 得出「别再这么干」的结论**（按主题追加：现象 / 根因 / 判据；常规落地流水不进仓库）。本册 §一–§五 | `test_repo_holds_no_process_documents`（登记核对） |
| 13b | `docs/踩坑记录-2.md` | 同上，本册 §六–§十。**分册规则：册数不限，每册 ≤ 500 行，写满就再开一册**（章节编号跨册唯一，源码注释的 `《踩坑记录》§N` 按编号查） | `test_repo_holds_no_process_documents` + `test_controlled_docs_stay_concise`（发现式扫 `踩坑记录*.md`） |
| 13c | `docs/踩坑记录-3.md` | 同上，本册 §十一–§十五（录屏 MF/DXGI、编译成本与测试分层、SinkWriter 节流、被否定的槽位池、moov 单点）。**分册规则：每册 ≤ 500 行，写满再开一册** | 同上 |
| 13d | `docs/踩坑记录-4.md` | 同上，本册 §十六起：录制内核（单文件方案）的失败根因链、音频内容驱动时间线、看门狗与限时收尾；**§十七 录制的结构性隔离**（采集/编码解耦 + 出进程隔离 `curseen-rec.exe`） | `test_repo_holds_no_process_documents` + `test_controlled_docs_stay_concise` |
| 13e | `docs/踩坑记录-5.md` | 同上，本册 §十七续起：I-beam 光标闪烁（GDI 直写复制纹理）、音频两路混音对齐与音画滞后、「硬件编码」谎报（允许≠启用）；**§17.15–17.19**；**§二十 覆盖层两套 DPI 换算源分家（特效不落鼠标）**；**§二十六 贴图长图放大图片不放大：系统跟踪尺寸收口（windui 补丁 095）** | 同上 |
| 14 | `docs/UI_COMPONENTS.md` | 面板行组件库（`card.rs` 正本）变化；脚本参数类型→控件映射变化；Lua 脚本契约 / 绘图指令 / 宿主 API 变化 | `test_repo_holds_no_process_documents`（登记核对） |
| 15 | `assets/README.md` | 资源目录版图（归谁 / 放什么）变化 | `test_repo_holds_no_process_documents`（登记核对） |
| 16 | `assets/Sounds/README.md` | 音频格式白名单 / 配置键 / 版权归属变化 | （凡动 `SUPPORTED_EXT` 必读） |
| 17 | `assets/KeyDisplay/README.md` | 键名映射表用法变化 | — |
| 18 | `assets/lua/HoverDot/README.md` | 悬浮标出厂脚本清单变化 | — |
| 18b | `assets/lua/README.md` | **Lua 脚本作者指南**（随 assets 分发）：四类 role 契约 / 回调签名 / 绘图指令 / 参数 schema / 颜色串口径 / 出厂样例清单 / 排错表变化 | —（同类参考：Pecia 的 `script/scripts/lua_api.txt`） |
| 18c | `assets/lua/API.md` | **Lua 脚本 API 权威参考**（随 assets 分发）：全局函数 / 环境变量 / 绘图指令 / `ctx.fs` / 回调契约 / 返回值契约变化 | —（与 `scripting/api_*.rs` 实际注册逐项对账） |
| 18e | `assets/lua/Lua脚本手册.html` | **随包 Lua 手册**（指南+API 合一）：由两份 .md 经 `temp/build_lua_manual.py` 生成，帮助卡「Lua 脚本手册」按钮直接打开；源 .md 变了要重跑生成脚本 | —（生成物，以 18b/18c 源文档为准） |
| 18d | `assets/manual/隐藏快捷键手册.md`（+ 同名 `.html`，随 assets 分发进安装目录） | **隐藏快捷键手册**：固定不可改的隐藏键清单变化 | —（每条须对照源码实装） |
| 19 | `examples/README.md` | 离屏探针增删（整目录职责变化） | （`Cargo.toml` 的 `[[example]]` 必须同步） |
| 20 | `开发指南.md` | 开发/发版流程、文档同步对照、提交前清单、HISTORY 登记规范、工程纪律变化 | `test_docs_checks_the_manifest_files_exist`（修改流程唯一正本） |

> **成品 `docs/` 只收这七份**：`DESIGN.md` / `DESIGN_ADOPTION.md` / `PLATFORM_NOTES.md` /
> `RELEASE_CHECKLIST.md` / `UI_COMPONENTS.md` / `踩坑记录.md` / `踩坑记录-2.md`。新增其它文档
> 要先在这里登记，否则根 `.md` 白名单门禁会红。（踩坑记录可继续分册：`踩坑记录-3.md`……登记一条即可。）

## 二、构建脚本（也是文档的一部分，改了要同步 `build/build_readme.md`）

| 文件 | 职责 |
|---|---|
| `build/full.bat` | 总入口（双击即用）：1 → 2 → 3 → 4 → 5 → 6，任一步失败即停 |
| `build/1_check_env.bat` | 只读工具链体检（cargo / rustc host 是否 MSVC / python / git / `.3rdParty/`）。`.3rdParty/` 缺失只报 WARNING（步骤 2 会补），不算环境失败 |
| `build/2_download.bat` | 按 `build/deps.txt` 下载第三方源码到 `.3rdParty/`（tar.gz → 校验 marker → promote）；已有同版本目录则跳过 |
| `build/3_patch.bat` | 把 `patches/<库>-<版本>/` **单向**（只 patches → .3rdParty）覆盖到 `.3rdParty/` |
| `build/4_build.bat` | `cargo build --release` |
| `build/5_test.bat` | **真门禁**：集成测试（`tests/` + `tests/probes/`）→ 单元测试（`--lib`），均带 `-- --test-threads=1` |
| `build/6_deploy.bat` | `deploy.py` 拷 exe + MD5 对账（其中会调 `gen_licenses.py`）；`sync_assets.py` 单向同步 assets；再复核许可文件齐备 |
| `build/_common.bat` | 共用路径变量与 python / tar / curl 解析（须 `call`，故意不 `setlocal`） |
| `build/deps.txt` | 第三方源码下载登记表（`name=url\|marker-file`，**ASCII-only**，步骤 2 消费） |
| `build/deploy.py` / `gen_licenses.py` / `sync_assets.py` / `gate.py` / `boot_check.py` / `check_deps.ps1` / `layering_scan.py` / `hygiene_scan.py` | 构建 / 部署 / 许可同步 / 按需门禁 / 启动冒烟 / 依赖体检 / 分层体检 / 静态异味体检 |
| `examples/gen_icon.rs` | 由矢量源 `assets/icons/icon*.svg`（+ `images/*.svg` 二维码）逐档光栅化出 `tray_<N>[_red].rgba`（托盘五档）+ `app.ico`（exe/窗口）+ 二维码 PNG；换图标/二维码只改 SVG 再 `cargo run --example gen_icon --features icon-gen` |
| `build/app.rc` | 图标资源脚本（`MAINICON` ← `assets/icons/app.ico`），由 `build.rs` 经 `rc.exe` 编译；无 SDK 时跳过 |

## 三、不入库的工作区文件（根 `temp/`，仅追溯、可随时清理）

| 位置 | 内容 |
|---|---|
| `../temp/` | 编译中间产物（`target/`）、近期真机探针脚本、一次性输出转储 |

> 提炼规则：工作区里得出**需要长期遵守的结论**时，立刻写进 `main/` 下对应文档
> （`AGENTS.md` / `docs/DESIGN.md` / `docs/PLATFORM_NOTES.md`），不要只留在 `temp/`。

## 四、由别的机制覆盖的文件

| 文件 | 说明 |
|---|---|
| `assets/lua/**` | 统一 Lua 脚本（唯一脚本根，递归；目录即来源） |
| `assets/Sounds/*.wav` | 声音素材（点击声音/键盘声音/拖尾音效共用） |
| `build/sync_assets.py` | `build.rs` 不拷 assets，部署时靠它同步 |
| `LICENSE` | AGPL-3.0 许可全文（随发布包分发，步骤 ⑥ 自动同步 `LICENSE` / `THIRD-PARTY-NOTICES.md` / `licenses/`）。★ 官方**逐字**、不加任何署名/说明（版权声明在 `README.md` 与 `Cargo.toml`）；受门禁校验，见 `tests/docs_consistency.rs` |
| `Cargo.toml` | `[[example]]` / `[[test]]` 登记决定了 `examples/` 与 `tests/probes/` 的可运行性 |

## 五、改动后的核对流程

**完整流程（每类改动的 checklist + 步骤，含 HISTORY 登记）见《开发指南.md》——
流程只有这一份，本节不再重复，防止两处漂移。**

本清单只负责"哪些文件受门禁约束、何时必须更新"；逐项核对在每次改动收尾时照
《开发指南.md》第三、四节执行。
