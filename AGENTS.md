# CurSeen 项目规则（AGENTS.md）

本文件对 CurSeen 仓库根目录（本文件所在 `main/` 的上一级）及其全部子目录生效。任何 AI 工具在此文件夹工作时必须遵守。

> 开发流程与提交前清单见《开发指南.md》；发行步骤见 `docs/RELEASE_CHECKLIST.md`；
> **外部规范 / 通用建议的裁决清单（采纳哪些、不采纳哪些、为什么）见
> `docs/DESIGN_ADOPTION.md`**——读过通用规范先查它，已裁决的条目不重新讨论。

---

## 一、临时产物规则（硬性）

1. **所有 AI 临时产物一律只放项目根 `temp/` 目录**，包括：
   - 调试/验证脚本（`.py` / `.ps1` 等）
   - 构建日志、命令输出（**禁止重定向到 `main/` 或项目根**）
   - 计划/进度/发现类文件（`task_plan.md`、`findings.md`、`progress.md` 等）
   - 退役源码备份（`temp/retired/`）
   - ★ **`temp/` 是一次性区**：脚本随时可能被清理，**照文档引用之前先确认文件还在**；
     不在了就按该条记录的判据重写一个——「文档里写过」不等于「脚本还在」。
2. **禁止向 `main/` 内写入任何日志、临时脚本或草稿文件**。`main/` 只放：
   源码（`src/`）、测试（`tests/`）、离屏探针（`examples/`）、构建脚本与辅助
   （`build.rs` + `build/`）、第三方补丁登记（`patches/`）、文档（`*.md` / `docs/`）、
   资源（`assets/`）。
   ★ **`main/` 是要上传 git 仓库的目录**：凡进 `main/docs/` 的文档必须有长期价值并在
   `DOCS_MANIFEST.md` 登记（方案 / 说明 / 踩坑记录都入库，受门禁核对）；一次性草稿 / 实测转储
   放根 `temp/`，不入库。「未受控文档进仓库」是本项目明令禁止的——半年后既无人维护，
   又会被误当成现行约定。
3. 固定工具**随仓库分发**（在 `main/build/` 下，随 git 走，供任何克隆者使用）：
   - `build/full.bat` —— 一键流水线（体检 → 下载 → 打补丁 → 编译 → 测试 → 部署，六步）
   - `build/gate.py` —— 按需跑测试组（`python build/gate.py config runtime`）
   - `build/layering_scan.py` —— 分层依赖体检（只读，`python build/layering_scan.py`）
   - `build/hygiene_scan.py` —— 静态异味体检（只读，`python build/hygiene_scan.py`）
   - `build/deploy.py` / `gen_licenses.py` / `sync_assets.py` / `boot_check.py` / `check_deps.ps1`
     —— 部署 / 许可随包同步 / 资源同步 / 冒烟 / 体检
   - `build/deps.txt` —— 第三方源码下载登记表（`2_download.bat` 消费，**必须 ASCII-only**）
   `temp/` 只放一次性诊断脚本与运行期产物，不视为固定工具。
4. 删除临时文件时只动 `temp/` 内自己创建的文件。

---

## 二、`main/` 目录纯净性（硬性）

| 允许 | 禁止 |
|---|---|
| `src/` `tests/` `examples/` `build/` `patches/` `docs/` `assets/` | 任何 `.log` / `.txt` 草稿 / `__pycache__` |
| `Cargo.toml` `Cargo.lock` `build.rs` | 编译中间产物（`.rs.bk`、备份等） |
| 随源码的文档（`*.md` / `docs/`） | 未在 DOCS_MANIFEST.md 登记的文档 / 一次性草稿（→ 根 `temp/`） |
| 构建脚本（`build/*.bat` + `build/*.py`） | 一次性诊断脚本（→ 根 `temp/`） |

> 第三方依赖 `windui` 位于 `.3rdParty/wind-ui-rust-0.30.0`（本地 path 依赖）。
> `.3rdParty/` 在仓库之外，**由 `main/build/deps.txt` + `2_download.bat` 取回上游源码、
> 再由 `3_patch.bat` 重放本仓库的补丁**——下载地址与补丁归档就是它存在的全部依据。
> **禁止**把 `main/` 里任何文件复制到 `.3rdParty/`；反向（`.3rdParty/` → `main/`）也不允许。

---

## 三、分层与依赖方向（强制）

```
core ← platform ← scripting ← modules ┐
   ↑                                   ├ app ← ui ← runtime
   └────────── update ─────────────────┘
```

由低到高依次为：`core → platform → scripting → modules → update → app → ui → runtime`。
**只允许低层不知道高层**，逆向依赖一律视为缺陷。

硬规则：

1. **`core/` 必须是纯逻辑**：不得 `use crate::platform` / `crate::modules` / `crate::runtime`
   / `crate::ui` / `crate::app`，也不得使用 `windui` 与 `windows` crate。
   - 需要跨界能力时用**依赖倒置**：由调用方注入（例：[`core::backdrop::Backdrop::capture`]
     把抓屏函数当作参数收进来，而不是自己调 `platform::screenshot::grab`）。
2. **`modules/` 不得 `use crate::ui` / `crate::runtime`**。
3. **`core/` 与 `modules/` 禁用 `windui` / `windows` 裸调用**（可测试性的根基）。
4. **`ui/` 不得 `use crate::runtime`**：面板经 [`crate::ui::PanelHost`] trait 向宿主取值
   （依赖倒置，对齐 Pecia 的 `core/UiBridge.h` + `ui/UiBridge.cpp`）。
5. **消息契约放在 `core::events`**（`InputEvent` / `UiMsg` / `UpdateEvent`）——目的是让
   生产方与消费方互不依赖、分层图无环。（同样取自 Pecia 把 `PipeProtocol.h` 放进 `core` 的手法。）

改动后必须跑分层体检与静态异味体检：

```bash
python build/layering_scan.py     # 期望输出：「违规 0 处」
python build/hygiene_scan.py      # 期望输出：「违规 0 处」
```

---

## 四、这台机器的执行环境（踩过的坑，硬性）

| 坑 | 对策 |
|---|---|
| **PowerShell 前台 stdout 取不到**（`Write-Output` 也空） | 要读输出就写进文件再 `Read`，或走 Python 脚本 `capture_output` |
| PowerShell 5.1 的 `>` 重定向写 **UTF-16**，`Read` 会报 binary | 显式 `-Encoding utf8`，或统一走 `main/build/*.py`（自带输出捕获） |
| PowerShell 的 `Remove-Item` 恒失效 | 删文件用 Python（`os.remove` / `shutil`）或 Bash `rm` |
| Bash 工具是 **git bash**：不能写 PowerShell 指令（`Out-File` / `Write-Output` 会 `command not found`） | 在 Bash 里就写 shell；需要 PowerShell 时用 PowerShell 工具 |
| cargo 偶发性 `os error 5 拒绝访问`（杀软/索引持锁） | 重跑；设 `CARGO_INCREMENTAL=0` 可少一类噪声 |
| 一条几十个文件的批处理命令会静默失败 | 复杂操作落成 Python 脚本再执行 |
| target-dir 由 `main/.cargo/config.toml` 指定为 `../temp/target` | 编译产物不在 `main/` 内，别去找 `main/target` |
| ★★ **`build.rs` 的 `rerun-if-changed` 指向不存在的路径**（如 `.git/HEAD`，而 `main` 是工作副本、不是 git 仓库）→ cargo 判"永远脏"，**每次** cargo 调用都重跑构建脚本、连带整个 crate 全量重编 | **新增 `rerun-if-changed` 前先确认目标真实存在**；现有那行已按存在性下发（别改回无条件下发） |
| exe **不可复现**：PE 头 `TimeDateStamp` 写的是**构建时刻** | 别用 MD5 判"代码有没有变"——要抹掉 PE 头那 4 字节再比哈希。`deploy.py` 的 `MD5_MATCH=True` 证明的只是"部署的就是刚编出来的那个文件" |
| 门禁的**成本结构**：测试本身只跑几秒，**慢的全是编译**（改任意一个 `.rs` ≈ 1m25s，过滤挡不住） | 攒一批改动再编一次，别"改一行编一次" |
| 按需检验用 `python main/build/gate.py <组名…>`（省的是**信号**不是时间） | 内层循环 `cargo check --release --tests`（**必须带 `--tests`** 才会检查 `#[cfg(test)]` 代码） |
| 面板截图：`PrintWindow` 抓这块分层窗返回 **0**（全黑）；`SetForegroundWindow` 在非前台进程调用时**静默被拒** | 口径：`SetForegroundWindow` 前**先敲一下 ALT**解锁前台锁 → 置顶 → **截完验像素**（面板正文三点够亮）再决定是否重截。找窗口要**枚举**后挑"可见+在屏+面积最大"，`FindWindowW` 会撞上同名的 160×28 隐藏窗 |
| ★★ **面板"点不动"不是分层窗的锅**：CurSeen 启动即自提权（High 完整性），**普通权限**脚本的 `SetCursorPos + mouse_event` 被 UIPI **静默丢弃**（键鼠钩子里连日志都没有），而键盘注入能进 | 真机验收脚本必须**自己提权重跑**（本机 `ShellExecuteW(None,"runas",…)` 静默成功、无 UAC），之后点导航/箭头/按钮才生效。四条硬性：① 全部步骤必须在**同一个**提权子进程里跑完（执行环境按 Job Object 回收派生进程，父进程一 `return` 整棵树被杀；父进程要等 `=== DONE ===` 标记，不是"报告文件存在"）；② 点击/截图前 `GetForegroundWindow()==h` 校验，不成就**跳过**（实测面板被全屏视频盖住时，盲注入会把点击送进用户窗口），收尾 `HWND_NOTOPMOST` 还原；③ 报告每条带**墙钟时间戳**，否则"按钮没变"是真是假说不清；④ 折卡片**从下往上点**（上面一折、下面整块上移，旧 y 就落到别的行） |

---

## 五、源码内的已知陷阱（改代码前必读）

### ★ 长度单位：物理像素 vs 逻辑像素

- `Window::position(x, y)` 是**物理像素**；`Window::new(title, w, h)` 是**逻辑 dp**。两者单位不同。
- Canvas 坐标是逻辑；GDI 冻结帧 / `ScreenRect` 是物理。
- 世界 → 窗口唯一换算 = `/dpi`（`runtime/overlay.rs` 的 `scale = 1/dpi` 就是这个约定）。
  **禁止在 consumer 里手写 `as i32` 直传**。
- `core::geometry::Rect`（`f32`，窗口内逻辑）/ `core::geometry::ScreenRect`（`i32`，虚拟桌面
  物理）**不可互相强转**。

### ★ `Sender` 是 windui 的，不是 std 的

`windui::prelude::Sender` 是 `std::sync::mpsc::Sender` 的**包装**，额外携带"唤醒 UI 线程一帧"
的 waker。写成 `use std::sync::mpsc::Sender` 会编译失败或静默丢失唤醒语义——表现为
"后台线程消息发出去了，但 UI 不重绘（更新进度条不动）"。详见 `src/ui/host.rs` 的注释。

### ★ 无边框窗的最外 8px 是缩放热区

`.frameless(true)` 在 `WM_NCHITTEST` 里把外圈 `RESIZE_BORDER_LOGICAL = 8` 判为缩放热区，
且**不看 `resizable`** → 全屏区域窗最外 8px 点不出选区（补丁 022-a 已修）。

### ★ 别用 `Element::window_drag()`

它返回 `HTCAPTION` 后鼠标消息进不了客户区，`PointerEvent::click_count`（双击）拿不到。

### ★ 单实例必须早仲裁

`curseen_ui::run()` 第一行就要 `windui::claim_instance(APP_ID)`，且**必须与**
`App::single_instance` 用的是同一个 `APP_ID`（不一致会导致本进程把自己判成二次实例、
窗口永不出现）。理由见 `src/lib.rs` 常量注释。

---

## 六、编码纪律

1. **禁止 `unwrap` / `expect` / `panic!`**（`test` 与 `main` 启动极早期豁免）；**禁止 `todo!`**。
   暴露给 Lua 的 API 尤其不得 panic（`rng` 空区间要兜底）。
2. 改 [`DrawCmd`](src/core/draw.rs) 必须同步改 `src/runtime/render.rs`——两边的命令集是一份契约。
3. 新增多于 60 行的逻辑必须带对应单测；纯逻辑优先（UI 相关的用离屏渲染探针）。
4. 注释写**为什么**而不是**做了什么**；每条非常规取舍都要留"踩过才知道"的理由。
5. **★ unsafe 必须写 `// SAFETY:`**（义务与证据，不是复述操作）。**指针类** unsafe
   （`transmute` / `from_raw_*` / `slice::from_raw_*` / `get_unchecked` / `copy_nonoverlapping`）
   一律要写，漏写 `python build/hygiene_scan.py` 第 6 查会红（硬门禁，阈值 0）。
   非指针类存量见 `docs/DESIGN_ADOPTION.md` D3，**新增的一律要写**。
6. **错误要带路径 / 操作**：配置读写这类"用户数据"路径禁止无上下文的自动转换，
   用 `CurSeenError::IoAt { path, op, source }`；只有上下文无关处才能 `From` 自动转。
   （同一底层错误出现在 ≥2 条语义不同的路径时，自动转换会让日志只剩 `io error: …`。）
7. **日志只打一次**：错误只传递，在事件入口和后台任务完成点统一记录——每一层都
   `log::error!` 会让同一次失败产生五条日志，读的人无从判断影响面。
8. **所有权签名**：要长期存储就收 `T`，只读就收 `&T`——**绝不让 `&T` 进函数再 `.clone()`**
   （隐藏成本，还剥夺调用方"这份数据你拿走"的选择权）。newtype 不实现 `Deref`。
9. 对第三方 `windui` 的任何修改都必须登记进
   **`main/patches/wind-ui-rust-0.30.0/PATCHES.md`**（编号递增，含背景 / 改法 /
   可复实验收表），并在被改文件里用 `★ CurSeen 补丁 0NN` 标注。
   `tests/docs_consistency.rs` 会扫源码里的标记、回查登记表有没有对应章节——
   漏登记直接红。升级依赖时按该目录从头重放。
   ★ 两条硬约束（同 `patches/README.md`）：**只增不减**（补丁章节永不删节）、
   **禁止反向覆盖**（不得把 `.3rdParty/` 的文件复制回 `patches/` 当归档——那可能是
   未打补丁的副本，会让归档残缺）。
10. **文档维护**：改了源码 / API / 目录结构，按《开发指南.md》第三节的对照表同步文档，
   并逐项核对 `DOCS_MANIFEST.md`；新增文档必须同时登记（`main/` 根 `.md` 有白名单门禁）。
   写"新测出来的结论"进现行文档，别只留在 `temp/`。

---

## 七、交付前核查清单（硬性）

按顺序完成，全部通过方可答复"完成了"。

| # | 项 | 命令 / 做法 | 通过判据 |
|---|---|---|---|
| 1 | 格式化 | `cargo fmt --check` | exit 0 |
| 2 | 静态检查 | `cargo clippy --all-targets` | exit 0，**第一方源码零 warning**。★ 现存 1 条 `unexpected cfg condition name: cargo_c` 来自第三方 path 依赖 `nnnoiseless`，不是本项目代码，别去修 |
| 3 | 测试 | `main\build\5_test.bat` | 全部 passed，**数量不少于上一版**（脚本内已带 `--test-threads=1`，别改成多线程） |
| 4 | 文档一致性 | 含于第 3 项（`tests/docs_consistency.rs`） | 全绿 |
| 5 | 分层体检 | `python build/layering_scan.py`（在 `main/` 下） | 「违规 0 处」 |
| 5b | 静态异味 | `python build/hygiene_scan.py`（在 `main/` 下） | 「违规 0 处」（硬编码绝对路径 / 调试残留 / `TODO:` 等） |
| 6 | 部署 | `main\build\6_deploy.bat` | `MD5_MATCH=True` `SIZE_MATCH=True`，且 `build\licenses\` 非空 |
| 6b | 一键 | `main\build\full.bat` | 体检 → 下载 → 打补丁 → 编译 → 测试（真门禁）→ 部署 **六步全过** |
| 6c | 从零（改过 `deps.txt` / `patches/` 时必做） | 把 `.3rdParty/` 改名腾空后跑 `full.bat` | 六步全过，且重建的 `.3rdParty/` 与腾空前的树**逐文件一致** |
| 7 | 快照留档 | 改动正在落地前备份 `main/` 到项目根 `.bf/<MMDD-HHMM> <说明>/` | 有 |
| 8 | 文档同步 | 改了源码就逐项核对 `DOCS_MANIFEST.md` | 清单里的文件都存在且描述成立 |

> 红线：本清单只检查、不提交。★ 本仓当前**没有 git**（`.git` 不存在），留档靠 `.bf/` 快照
> （见《开发指南.md》§二）；若日后接入 git，`commit` 必须先取得用户明确许可。
