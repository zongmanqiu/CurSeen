# 工程总纲采纳清单

> 来源：项目根 `Rust 软件工程设计总纲.md`（桌面 GUI / Win10+Win11 / Rust）。
> 性质：那份文档是**通用建议集**，本文件是它对 CurSeen 的**裁决结果**——哪些采纳、哪些不采纳、
> 采纳的落到哪个文件。半年后有人问"这条规矩哪来的"，答案在这里。
>
> 裁决口径（三条）：
> 1. **只采纳能落地成门禁或代码的**。写不进 `hygiene_scan.py` / `layering_scan.py` /
>    类型 / 测试的，就只是愿望，不进本表。
> 2. **CurSeen 不是那段总纲的默认画像**：它不驻托盘、不做网络服务、不用 tokio、
>    **便携化**（运行期一个字节都不写到 exe 同级目录之外）、单 crate + 目录分层。
>    凡以"五层 workspace / `%APPDATA%` / 常驻后台服务"为前提的条目，一律按不采纳处理。
> 3. 不采纳也要写理由——**不写理由的"不做"会在半年后被当成漏项重新做一遍**。

---

## 一、已采纳并落地（本次改了源码 / 脚本）

| # | 总纲条目 | 章节 | 落地前的现状 | 落地位置 | 判据 |
|---|---|---|---|---|---|
| A1 | VERSIONINFO **七字段必填**（缺厂商信息 = 杀软重点关照对象） | §11.1 | 已有 CompanyName / FileDescription / FileVersion / InternalName / OriginalFilename / ProductName / ProductVersion，**缺 `LegalCopyright`** | `build.rs::version_rc_text()` | 属性页「版权」非空 |
| A2 | 构建后**校验版本资源非空**（防止有人改坏 `build.rs` 或机器上没有 `rc.exe`，exe 悄悄变成"三无文件"） | §16 | 无任何校验；`rc.exe` 缺失时 `build.rs` 只打一条 warning 就继续 | `build/deploy.py` 报告新增 `VERINFO=ok/missing` | 有 `rc.exe` 的机器上 `VERINFO=ok`；把 `build/app.rc` 挪走重编 → `missing`（失败即停） |
| A3 | 配置原子写 = **临时文件 → fsync → 原子替换 → 父目录 fsync**；目录 fsync 失败**降级告警，不得反噬整条保存路径** | §7.2 | 只有 `fs::write(tmp)` + `fs::rename(tmp, path)`，**两次 fsync 都没有** | `core/config.rs::save()` 加文件 fsync；`platform/win_util.rs::sync_parent_dir()` 做目录 fsync，经 core 的注入钩子调用（core 不得碰 Win32） | 单测：写盘后文件与父目录都 sync 过；目录 sync 失败时保存**仍成功** |
| A4 | **错误要带路径 / 操作**：`From<io::Error>` 一路自动转换会把"从哪来"抹掉 | §9.2 | `CurSeenError::Io(std::io::Error)` + `From` 自动转换，配置读写出错时日志里只有 `io error: ...` | 新增 `CurSeenError::IoAt { path, op, source }`，配置读写两条路径改用它 | 单测：`Display` 串含路径与操作名 |
| A5 | **日志头必须包含**版本 + git commit + OS + 显示器 + 完整性级别 + 启动参数 | §10 | 只有一行 `CurSeen vX 启动` | `platform/win_util.rs::log_env_banner()`（OS / 显示器 / 完整性 / DPI 都在这里取），`main.rs` 在 `init_logging()` 之后调用 | 启动日志前两行能看到版本 + git、系统版本与 build、显示器台数 / 虚拟屏 / 主屏缩放、是否提权、启动参数 |
| A6 | unsafe 块必须写 `// SAFETY:`（**义务与证据**，不是复述操作） | §15 | 408 个 `unsafe {` 只有 20 处注释 | ① 全部**指针类** unsafe（`transmute` / `from_raw_*` / `slice::from_raw_*` / `get_unchecked` / `copy_nonoverlapping`）补齐；② `build/hygiene_scan.py` 新增第 6 查 | `python build/hygiene_scan.py` 第 6 查「违规 0 处」；故意删掉一处 SAFETY 注释 → 变红 |

---

## 二、已采纳为纪律（写进 `AGENTS.md`，本次不改代码）

这些条目**当前已经成立或无法自动化**，只做约定，不进改动清单。

| # | 总纲条目 | 章节 | CurSeen 现状 / 约定 |
|---|---|---|---|
| B1 | **UI 线程只做收消息 / update / 渲染**；文件 I/O、网络、重计算全下放 | §6.1 | 已成立：HTTP 走具名 worker（`ai_chat` / `update/task`），日志走独立写线程。**新增任何阻塞调用必须先确认不在 UI 线程** |
| B2 | 长任务可取消：具名 worker + `CancellationToken` + 关停四步 | §6.9 | 已成立（裸 `thread::spawn` 一律 `Builder` + `.name()`）。**新增长活照此办理** |
| B3 | 进度回投**节流 50~100ms**，有界 channel，满了丢帧 | §6.4 | 已成立（帧预算纪律：park 态不出帧、新增常驻功能过帧门三查） |
| B4 | **日志只打一次**（每层都 `error!` 是反模式） | §9.8 | 约定：错误只传递，在事件入口与后台任务完成点记一次 |
| B5 | 所有权签名：`&T` 进来就别在函数里 `.clone()` | §4.1 | 约定：要长期存储就收 `T` |
| B6 | newtype **不实现 `Deref`**（那是把约束又打开） | §3.1 | 当前 0 处，**守住** |
| B7 | 错误三分类（外部 / 用户 / 内部），响应管道不同 | §9.3 | 约定：用户错误弹提示**不重试**；外部错误有界重试 |
| B8 | 单文件 ~800 行上限 | §16 | **已知超标**（`core/config.rs`、`platform/win_util.rs` 等），记为债务；**新文件不得超** |
| B9 | 每次看到 `Arc<Mutex<T>>` 过五问 | §6.8 | 当前 14 处，**只准降不准升**；新增必须写明为什么不能消息传递 |
| B10 | 剪贴板 `OpenClipboard` 占用时间极短、监听用 `AddClipboardFormatListener` | §8.8 | 已成立（门禁：`hygiene_scan.py` 第 4 查 + `with_clipboard` / `lock_clip`） |
| B11 | 跨 FFI 的 unwind 是 UB，回调边界拦 `catch_unwind` | §9.7 | 已成立（钩子 / 脚本宿主 11 处） |

---

## 三、记录但不采纳（含理由，别再捡起来）

| # | 总纲条目 | 章节 | 不采纳的理由 |
|---|---|---|---|
| C1 | 拆五层 workspace（`domain` / `application` / `platform` / `ui` / `app_main`） | §2.1 | CurSeen 是单 crate + 目录分层，**依赖方向已经物理化**：`build/layering_scan.py` 扫 `use crate::` 与 `windui`/`windows` 裸调用，违规即红。拆 crate 只能再加一层物理保证，代价是构建时间与体积（本项目 `opt-level="z"`、体积优化已终局） |
| C2 | `Clock` / `UuidGen` 端口注入，领域函数不读全局时钟 | §2.2 / §0.4-D′ | 录屏、特效、防抖全靠**真实时钟**，把 `Instant::now()` 参数化要穿透几百个调用点，收益（防抖可确定性单测）远小于改造面。**不采纳** |
| C3 | 快照测试（`insta` 对 `view(state)` 做快照） | §13 | 违本项目「克制引入新库」纪律；且 UI 是即时模式自绘，没有 ViewModel 这层可快照的稳定结构 |
| C4 | 无障碍 UIA Provider / IME 候选框跟随 / 本地化字符串外置 | §12 | 面板是自绘即时模式 UI，接 UIA 要为每个控件写 Provider，成本数量级高于收益；当前无多语言需求 |
| C5 | 公共类型加 `#[non_exhaustive]` | §4.4 | **本仓是应用层不是库**，没有下游 crate 会 `match` 我们的类型；加了反而是关掉自己的穷尽性重构安全网 |
| C6 | 五层 `update(&mut state, msg) -> Vec<Effect>` 纯状态机 | §5.1 | 架构已在运行（帧驱动 + 事件队列），整体改 MVU 等于重写 `runtime`。**它的价值已由"逻辑下沉 + 分层门禁"部分兑现** |
| C7 | tokio / 异步运行时 | §6.2 | 无并发网络 I/O；`std::thread` + channel 已够 |
| C8 | 配置写 `%APPDATA%` / SQLite WAL / 托盘 / 开机自启 / 文件关联 / 自动更新签名 | §7 / §8 / §11 | **与便携化铁律冲突**：运行期一个字节都不写到 exe 同级目录之外，也不读 `%APPDATA%`。CurSeen 的"配置损坏回退默认"（§7.3）与"原子写"（§7.2）照抄，落盘位置不照抄 |
| C9 | 全局钩子 / 注入一律不用 | §11.3 | CurSeen **必须**用键鼠钩子（这是产品本体）。补偿手段已做：签名 + VERSIONINFO + 不加壳 + 不写全局注入 |
| C10 | 禁止 `once_cell` / `lazy_static` 全局单例 | §2.4 | 允许**自举类**单例（日志 `LOGGER`、本次新增的目录 fsync 钩子），除此之外新代码不得再加全局可变状态 |

---

## 四、已知缺口（本次不动，风险写明）

| # | 缺口 | 风险 | 为什么这次不做 |
|---|---|---|---|
| D1 | **未接管系统事件**：`WM_QUERYENDSESSION` / `WM_POWERBROADCAST` / `WM_DISPLAYCHANGE` / `WM_WTSSESSION_CHANGE` | 录屏中关机 → `moov` 来不及写 → 产物不可用；录屏中睡眠唤醒 → 采集设备失效 | 需要自建消息窗 + 泵 + 与 windui 主循环协作，属结构性改动，单独立项 |
| D2 | 单文件行数超标 | `core/config.rs` 4600+ 行、`platform/win_util.rs` 1300+ 行，改动时定位困难 | 拆分要动几百个引用点，单独立项 |
| D3 | 非指针类 unsafe 缺 `// SAFETY:` | 读代码的人无从判断前提 | 存量 300+ 处，逐步清零；**新增一律要写**（A6 门禁只卡指针类，是因为那才是 UB 高发区） |
| D4 | 无"上次是否干净退出"的启动自检 | 崩在启动早期时无从分辨 | 需要落 `running.marker`，与便携化落盘规则一起评估 |

---

## 五、维护

- 再读那份总纲（或任何通用规范）时，先搜本文件：已裁决的条目**直接照本文件执行**，不重新讨论。
- 采纳 / 不采纳的结论变了，必须**同时**改本文件的理由栏和对应落地处，不能只改一边。
