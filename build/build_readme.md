# 构建流程总说明

> `main/build/` 是**源码**（随仓库分发），不是构建产物。
> 真正的产物在 `<项目根>/build/`（exe + 许可文件 + assets）与 `<项目根>/temp/target/`
> （cargo 中间文件），两者都在仓库之外。

## 一、六步流水线

| 步骤 | 脚本 | 干什么 | 失败会怎样 |
|---|---|---|---|
| ① | `1_check_env.bat` | **只读**体检：cargo / rustc host 是否 MSVC、python、git、`.3rdParty/` 两个 path 依赖在不在（缺了只报 WARNING，因为 ② 会补） | 工具链缺项 → 退出 1；`.3rdParty/` 缺失不算失败 |
| ② | `2_download.bat` | 按 `deps.txt` 下载第三方源码到 `.3rdParty/`（tar.gz，解压后校验 marker 才算成功）；已有同版本目录则跳过 | 退出 1 |
| ③ | `3_patch.bat` | 把 `main/patches/<库>-<版本>/` **单向**覆盖到 `.3rdParty/<库>-<版本>/` | 退出 1 |
| ④ | `4_build.bat` | `cargo build --release`（在 `main/` 下跑，命中 `main/.cargo/config.toml`） | 退出 1 |
| ⑤ | `5_test.bat` | **真门禁**：集成测试（`tests/` + `tests/probes/`）→ 单元测试（`--lib`），均单线程 | 退出 1，`full.bat` 就此停住，不会部署 |
| ⑥ | `6_deploy.bat` | `deploy.py` 拷 exe + MD5 对账，顺带调 `gen_licenses.py` 把许可同步进 `build/`，再验 `assets/` | 退出 1 |

一键：`full.bat`（双击即可，按顺序 ①→⑥，任一步失败即停）。

> **②③ 让这个流水线能从零跑**：全新克隆出来的 `.3rdParty/` 是空的（它在仓库之外），
> ② 按 `deps.txt` 把上游源码拉回来，③ 再把 CurSeen 的补丁系列重放上去。
> 所以 `full.bat` 不需要任何"先手工准备依赖"的前置步骤。

按需单跑：

```bat
main\build\1_check_env.bat   REM 新机器先跑这个
main\build\2_download.bat    REM .3rdParty\ 空了 / 想换版本
main\build\3_patch.bat       REM 改过 main\patches\ 之后
main\build\4_build.bat       REM 只改了代码、想快点看结果
main\build\5_test.bat        REM 提交前
main\build\6_deploy.bat      REM 只是想刷新 build\
```

## 二、第三方源码从哪来（`deps.txt`）

`.3rdParty/` 里放的是**打了补丁的上游源码**，不能从 crates.io 取得，所以下载地址被
登记成一张表：`main/build/deps.txt`。

```
# 每行：name=url|marker-file
wind-ui-rust-0.30.0=https://codeload.github.com/huanfeng/wind-ui-rust/tar.gz/refs/tags/v0.30.0|src/core.rs
```

- `name` 必须与 `.3rdParty/<name>/` 目录名、`main/Cargo.toml` 的 path 依赖**逐字对应**
  （`tests/docs_consistency.rs` 会核对）。
- `marker-file` 是"这次下载完整吗"的**唯一探针**：解压到临时目录后必须能看到这个相对
  路径，才允许 promote 成 `.3rdParty/<name>/`。半截包会被挡住。
- 文件必须是 **ASCII-only**：`cmd.exe` 按 OEM 码页（本机 GBK）解析它，写中文注释会乱码，
  拆出来的 url / marker 全错（门禁已钉住）。
- 已存在同版本目录时跳过；要强制重下：`set CURSEEN_FORCE_DOWNLOAD=1`。

> 用 `codeload.github.com/.../tar.gz/refs/tags/vX.Y.Z` 而不是
> `github.com/.../archive/refs/tags/vX.Y.Z.zip`：前者是归档主机，代理下更稳，
> 且返回 tar.gz 可以直接喂给 `%SystemRoot%\System32\tar.exe`（bsdtar）。
> ★ **不要用 PATH 里的 `tar`**：从 Git Bash 启动时 PATH 首个是 GNU tar，它会把
> `E:\...` 当远程主机（`Cannot connect to E: resolve failed`）。`_common.bat` 已按
> `%SystemRoot%` 优先解析。

## 三、目录与产物位置

```
<项目根>\
├── main\                      ← 上传单位（源码 + 现行文档 + 构建脚本 + 资源）
│   ├── build\                 ← 本目录：构建脚本（源码）
│   ├── patches\               ← 第三方补丁登记 + 补丁后完整文件（源码）
│   ├── src\ tests\ examples\ assets\
│   ├── .cargo\config.toml     ← target-dir = ../temp/target
│   └── ...
├── .3rdParty\                 ← path 依赖（不在仓库里；由 ② 下载、③ 打补丁）
│   └── wind-ui-rust-0.30.0\
├── temp\target\               ← cargo 中间产物（不在仓库里）
└── build\                     ← 可直接分发：exe + LICENSE + licenses\（含全部第三方
                                  许可文件） + assets\ + 运行期数据（不在仓库里）
```

> **为什么中间产物要挪出去**：`cargo` 只从**当前工作目录**向上查 `.cargo/config.toml`，
> 与 `--manifest-path` 无关。所以从项目根发起的构建读不到 `main/.cargo/config.toml`，
> 会在源码树里长出 `main/target`（几百 MB）。项目根另有一份 `.cargo/config.toml`
> 兜底，两处指向同一个 `temp/target`。

## 四、许可证随包分发（步骤 ⑥ 的一部分）

`build/` 是**可直接拿出去分发**的目录。AGPL-3.0 要求随二进制保留自身许可全文 + 全部
第三方许可声明，所以步骤 ⑥ 会调 `gen_licenses.py` 把下列内容同步进去
（copy-if-different，幂等）：

```
build/
├── curseen.exe
├── LICENSE                     ← main/LICENSE（AGPL-3.0 全文）
├── licenses/                   ← 第三方许可全部在这一层
│   ├── THIRD-PARTY-NOTICES.md  ← main/THIRD-PARTY-NOTICES.md（清单 / 补丁变更声明）
│   ├── THIRD-PARTY-LICENSES.txt← 全部 crates.io 依赖的许可原文合并（脚本自动生成）
│   ├── windui-LICENSE-MIT.txt
│   └── windui-LICENSE-APACHE.txt
├── assets/
└── config.json
```

- 源缺失（例如清空 `.3rdParty/` 后直接跑 ⑥）→ `gen_licenses.py` 返回 1 → `deploy.py`
  返回 1 → 本步失败。**不允许**静默产出一个没有许可声明的发布目录。
- crates.io 依赖的许可原文由 `gen_third_party_licenses.py` 从 cargo registry 汇总成
  `licenses/THIRD-PARTY-LICENSES.txt` 单文件随包分发（MIT/Apache/BSD 都要求**全文**
  跟着拷贝走，"在 cargo 缓存里"对拿到包的用户不成立）。只有两个本地 path 依赖
  （windui）的原文是从 `.3rdParty/` 逐字拷贝的单独文件。

## 五、环境变量开关

| 变量 | 作用 |
|---|---|
| `CURSEEN_CHAINED=1` | 抑制末尾的"按任意键"（`full.bat` 调用子脚本时自动设） |
| `CURSEEN_SKIP_TESTS=1` | 跳过第 ⑤ 步（本地快速迭代用，**提交前必须跑全**） |
| `CURSEEN_FORCE_DOWNLOAD=1` | 第 ② 步忽略已有目录，强制重下全部依赖 |
| `CURSEEN_CARGO_ARGS` | 追加给 `cargo build` 的参数，如 `-v` |

## 六、常见坑（都踩过）

1. **改了 `.rs` 后编译慢**（≈1m25s）不是测试慢——**测试只跑几秒**。慢的全是编译。
   攒一批改动再编一次，不要"改一行编一次"。
2. **`build.rs` 的 `rerun-if-changed` 指向不存在的路径**会让 cargo 判定"永远脏"，
   每次调用都全量重编。新增前先确认目标存在。
3. **exe 不可复现**：PE 头带构建时间戳，MD5 跨构建不可比。`deploy.py` 的 `MD5_MATCH`
   只证明"部署的是刚编出来的那个文件"。
4. **cargo 偶尔报 `os error 5 拒绝访问`**（杀软/索引持锁 `target`）：重跑即可；脚本里已设
   `CARGO_INCREMENTAL=0` 少一类 incremental 目录噪声。
5. **`.bat` 必须 CRLF**（见 `.gitattributes`）：LF-only 会被 cmd 误解析，报出与真实问题
   毫不相干的错误。用 `Edit`/`Write` 改过 `.bat` 后，务必确认换行没被规整成 LF。
6. **`.bat` 里 `echo` 的半角括号必须写成 `^(` / `^)`**：cmd 不会把 `( ... )` 块里 echo
   文本中的 `)` 当成嵌套——第一个 `)` 就提前关掉块。同行还有字 → 直接语法错；在行尾 →
   静默把块内后续语句变成**无条件执行**（后果是整个流水线每跑必停在第 ① 步，输出却像
   正常结束）。
   门禁 `test_build_batch_echo_lines_escape_parentheses` 已钉住。
7. **别只跑 `cargo test --release --lib`** 当全部门禁——它会跳过 `tests/` 下的
   `docs_consistency.rs`（文档腐化门禁）。第 ⑤ 步两层都跑就是为此。
8. **`cargo test` 必须带 `-- --test-threads=1`**：本机默认多线程会因杀软持锁新链接的
   测试二进制而卡住（看着像死机）。单线程数秒跑完，第 ⑤ 步已这么写。

## 七、python 辅助脚本（本目录内）

| 脚本 | 职责 |
|---|---|
| `deploy.py` | 拷 `temp/target/release/curseen.exe` → `<项目根>/build/`，MD5 + 大小对账；顺带调用 `gen_licenses.py`；报告写 `temp/deploy_report.txt` |
| `gen_licenses.py` | 把 `LICENSE` / `THIRD-PARTY-NOTICES.md` / `licenses/`（自 `.3rdParty/` 逐字拷贝）同步进 `build/`；源缺失即返回 1；报告写 `temp/licenses_report.txt` |
| `sync_assets.py` | `main/assets/` → `<项目根>/build/assets/` **单向**同步（只增和覆盖，**从不删**——`build/assets/` 是运行期数据目录，用户会在里面放自己的脚本/音效） |
| `gate.py` | 按需跑测试组（`python build/gate.py config runtime`），省的是**信号**不是时间 |
| `layering_scan.py` | 分层体检（只读，`python build/layering_scan.py` → 「违规 0 处」） |
| `hygiene_scan.py` | 静态异味体检（只读，`python build/hygiene_scan.py` → 「违规 0 处」）：硬编码绝对路径 / 调试残留（`println!` 等）/ 未完成标记（`TODO:`·`FIXME:`·`XXX:`）/ 裸 `OpenClipboard` / struct literal 里的临时宽字符缓冲 / **指针类 unsafe 缺 `// SAFETY:`** / **本机真实路径泄漏（全树，第 1~6 查之外唯一扫 `main/` 全部文本文件的检查）**；`#[cfg(test)]` 之后与纯注释性提及自动豁免 |
| `boot_check.py` | 启动冒烟：跑一次 exe，确认日志里没有 `[FATAL]` |
| `check_deps.ps1` | 依赖体检（PowerShell） |
| `deps.txt` | 第三方源码下载登记表（② 消费，格式见第二节；ASCII-only） |
| `examples/gen_icon.rs` | **换图标 / 二维码的唯一入口**（不是本目录的脚本，是 `examples/`）：`cargo run --example gen_icon --features icon-gen`，由 `assets/icons/icon*.svg` 与 `assets/images/*.svg` 光栅化出全部位图产物（托盘五档 `tray_<N>[_red].rgba`、`app.ico` 12 档、二维码 PNG 带静默区）。依赖 `resvg` 是 **optional**（默认不编译，不进 exe）；**不改图**：源图画什么颜色就是什么颜色，唯一的处理是光栅化 |
| `app.rc` | 图标资源脚本（资源名 `MAINICON`，windui 窗口类认的就是这名）。由 `build.rs` 调 Windows SDK 的 `rc.exe` 编译；**机器上没有 SDK 时构建跳过它**，exe 退回系统默认图标，不会构建失败 |
