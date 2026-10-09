# 版本发行流程清单（RELEASE_CHECKLIST.md）

> 发版时按顺序逐条打勾。改了流程就改本文件（见 `DOCS_MANIFEST.md`）。

## 一、版本号与文案同步

- [ ] `Cargo.toml` → `[package].version`（**唯一权威来源**，关于页 / 托盘 tooltip /
      exe 资源 / 自动更新全部编译期跟随它，无需手改别处）
- [ ] `Cargo.toml` → `description`（标语口径：与 README 主标题、托盘 tooltip、
      关于页副标题同源——2026-10-08 定稿「键鼠所至 · 屏幕所书 · 不止所见」）
- [ ] `README.md` → 下载链接文字里的版本号（"下载最新版 x.y.z"，此处是手写字面量）
- [ ] `HISTORY.md` → 顶部把 `（开发中）` 小节定为正式版本号，并把该节**压缩为面向使用者的
      功能 / 修复要点**（旧版本保持要点级，版本再多也不超篇幅上限）
- [ ] 复核 `update/version.rs` 读的远端地址仍有效（运行期版本号来源）

## 二、质量门禁（按顺序，全绿才继续）

- [ ] `main\build\1_check_env.bat` —— 工具链体检（只读）
- [ ] `main\build\2_download.bat` —— 按 `deps.txt` 下载第三方源码到 `.3rdParty/`
- [ ] `main\build\3_patch.bat` —— 把 `patches/` 单向覆盖到 `.3rdParty/`（重放补丁）
- [ ] `main\build\4_build.bat` —— `cargo build --release`
- [ ] `main\build\5_test.bat` —— **真门禁**：集成测试 + 单元测试；失败即停
- [ ] `main\build\6_deploy.bat` —— 拷 exe + 同步 assets + 同步许可（`MD5_MATCH=True` `SIZE_MATCH=True`）

等价的一键命令：`main\build\full.bat`。

★ 本版本若动过 `deps.txt` / `patches/`，必须再跑一次**从零构建**：
把 `.3rdParty/` 改名腾空 → `full.bat` → 六步全过，且重建的 `.3rdParty/` 与腾空前的树
逐文件一致（否则"从零重建"只是纸面能力）。

补充检查（发版前手动过一遍，脚本在 `main/build/` 下）：

- [ ] 分层体检：`python build/layering_scan.py`（在 `main/` 下）→ 「违规 0 处」
- [ ] 静态异味：`python build/hygiene_scan.py`（在 `main/` 下）→ 「违规 0 处」
- [ ] 启动冒烟：`python main/build/boot_check.py` → 日志里无 `[FATAL]`
- [ ] 文档一致性：含于第 5 项（`tests/docs_consistency.rs`）

## 三、文档同步

- [ ] `DOCS_MANIFEST.md` —— 逐项核对（改了源码就有对应的文档要改）
- [ ] `目录结构说明.md` —— 增删文件/目录、改职责、分层变动都要登记
- [ ] `开发指南.md` —— 流程 / 清单 / 检查项变化时（修改流程唯一正本）
- [ ] `README.md` —— 顶层目录树 / 构建方式 / 技术栈
- [ ] `THIRD-PARTY-NOTICES.md` —— 依赖版本变化（下载地址同步 `build/deps.txt`）
- [ ] `HISTORY.md` —— 顶部追加本版本小节
- [ ] `patches/wind-ui-rust-0.20.0/PATCHES.md` —— 本版本动过 windui 就补章节（同步归档改后完整文件）

## 四、打包

- [ ] 产物取自 `<项目根>/build/`：`curseen.exe`（录制载体 = **进程内线程**，只有一个
      exe；`curseen-rec.exe` 子进程方案已废弃，deploy.py 会清掉 build/ 里遗留的旧副本）
      + `assets/` + `LICENSE` +
      `THIRD-PARTY-NOTICES.md` + `licenses/`
- [ ] **许可文件随包分发已自动化**：步骤 ⑥ 同步并复核三者齐备，缺一即失败
      —— 直接整目录取用即可，**不要**再从 `main/` 手工复制、也不要手工编辑 `build/` 里的副本。
      口径与理由见《开发指南.md》§八；机制见 `build/build_readme.md` §四
- [ ] `LICENSE` 为官方逐字全文、UTF-8、**一个字都不许多**（门禁
      `tests/docs_consistency.rs::test_license_is_verbatim_agpl3_and_utf8` 已校验编码 + 官方锚点
      + `build/LICENSE` 与 `main/LICENSE` 逐字一致）
- [ ] 确认 `build/config.json`、`build/log/` 是**运行期数据**，不进发布包
      （便携版把它们留在程序目录，用户删目录即卸载干净）
- [ ] 体积复核（`opt-level = "z"` 是刻意的；渲染内核单独提到 3，见 `Cargo.toml` 注释。
      体积策略全貌 / 实测终点判据：`docs/DESIGN.md` §10——无新硬需求不重启体积优化）

## 五、真机验收（自动化之外，人手过一遍）

> 本节的定位：**跑一遍真实交互**。能自动化的部分已经在门禁 / 探针 / 单测里，本节只
> 补它们补不了的（真实窗口、真实输入、真实托盘）。每条末尾标了它的自动判据在哪——
> 标「仅人手」的是自动化确实兜不住的（真窗口层级 / 焦点 / 系统光标），不要为了
> 自动化而自动化（真机自动化必须独占机器，见 `PLATFORM_NOTES.md` 第四节）。

- [ ] 托盘图标 → 开面板（`Ctrl+Shift+D` 是**开关**）—— 仅人手
- [ ] 拖尾 / 点击效果 / 击键回显 / 击键音效 —— 自动：`probe.py run soak_link`（真机联动）
- [ ] 截图（普通 / 选区编辑 / 贴图）、GIF 录制与裁剪、MP4 录屏 —— 自动：
      `tests/record_smoke.rs`（门禁）、`capture_smoke`（真机）、`shot_edit.rs` 与
      `core/gif_trim.rs` 单测；**选区编辑的存盘 / 取消路径仅人手**
- [ ] 屏幕书写（保存 = 另存为；退出 = 默认位置留档）—— 仅人手
      （渲染有 `render_probe` / `shape_probe`，存盘与留档落点没有自动判据）
- [ ] 画中画 / 全屏放大（「快捷键」出厂留空，配置后一键切换；Esc 退全屏；倍率 = Ctrl+Win+滚轮）
      —— 自动：`pip_probe` / `pip_zoom_probe` / `fullscreen_zoom_probe` / `rec_pip_share`
- [ ] 截图与贴图（三把快捷键出厂**全留空**；贴图右键菜单不**被贴图窗盖住**、子菜单不盖父菜单
      ——靠屏幕右缘再验一遍）—— 仅人手（窗口层级与遮挡自动化兜不住）
- [ ] 「图片操作」：贴图右键**总有**「图片操作」子菜单（这一级**恒挂载**，撤掉 = 用户以为功能坏了）。
      里面有条目 ⟺ 该脚本既被 `assets/lua/scripts/` 磁盘扫到、又在脚本页勾开了
      （唯一判据 `LuaExtSettings::role_is_active("image", id)`，
      过滤点 `runtime::Runtime::lua_ext_menu_items`）。
      出厂全关 → 子菜单是空壳 + 一条置灰的「（暂无脚本）」；把目录里的脚本文件再挪走 → 子菜单
      仍是空壳，**不能整个消失**
      —— 自动：`pin.rs::lua_submenu_rows_keep_the_entry_even_when_empty`（空表恒留一行
      置灰占位 + 命令号 0；有脚本时命令号与下标严格对齐）
- [ ] MP4 录屏默认带声音：出厂「系统声音 / 麦克风」都是**跟随系统默认**；「麦克风降噪」出厂值是**中**。
      「录屏」「录制」「裁剪」三把快捷键出厂留空，验收前在面板里各配一把
      —— 声音自动：`audio_probe` / `audio_e2e` / `monitor_probe` / `denoise_probe`；
      出厂键位留空有 config 单测钉住，验收本身仅人手
- [ ] 剪贴板历史（`F4`）：文本 / 图片 / 文件条目齐备；点行粘贴进记事本与 QQ/TIM
      **到货**（QQ/TIM 是焦点恢复特异环境，必测）；面板可见时悬停**不出现启动转圈**
      （2026-10-07 修的「应用启动中」锁存，回归点）；关闭 CurSeen 后系统剪贴板
      恢复正常（屏蔽格式不残留）—— 自动：`clip_focus_restore` / `qq_paste`（真机）
      + `clipboard_history.rs` 单测；**悬停转圈与退出后格式不残留仅人手**
- [ ] 设置页两张**合并卡**（2026-10-07 新结构）：「功能开关」=「模块开关」+「脚本开关」
      两段、「快捷键」=「模块快捷键」+「脚本快捷键」两段；段名是**暗红加粗**、与数据行
      **同高**（行距在段名处不能塌），段间有横线；脚本段为空时给的是提示文字而非空框
      —— 仅人手（排版判据要真机截图；卡 id 与行序有 ui 单测钉住）
- [ ] 设置页「系统 > 管理员权限」行名随状态四态显示：`(未启用)` / `(重启后有效)` /
      `(重启后取消)` / `(已启用)`；勾选后**重启**才生效，勾上那一次启动会弹 UAC 确认；
      不勾则普通权限启动（出厂值）
      —— 自动：`global.rs::system_card_wires_admin_launch_toggle_and_hint`（四态文案齐备）
      ；UAC 弹窗与重启生效仅人手
- [ ] 自动更新：检查 → 下载 → 安装 —— 仅人手
- [ ] 退出后系统光标恢复正常（全局隐藏光标那条路径的兜底）—— 仅人手
      （`hide_cursor` 的状态机没有对外读取口，且真调 `hide()` 会动用户的系统光标）

> 真机自动化必须**独占机器**（见 `PLATFORM_NOTES.md` 第四节）。

## 六、上传与发行

- [ ] 仓库只提交 `main/`（`.3rdParty/` `temp/` `build/` 都在仓库外）
- [ ] 打 tag / 上传发行包
- [ ] 自动更新地址与实际下载地址一致

> 红线：本清单只负责检查。若日后接入 git，`commit` / `push` 必须先取得用户明确许可。
