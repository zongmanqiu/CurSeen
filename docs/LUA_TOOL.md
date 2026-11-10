# Lua 工具台（发行版内建）

> 给**外部 AI agent 与 CI** 用的脚本入口：不开窗、不提权、不抢单实例，把"这个脚本对不对"
> 变成一个可判的 `pass / fail`。实现见 `src/runtime/lua_tool/`。
>
> 存在理由：用户手上只有发行版，没有 cargo 也没有测试套件。所以判据必须由 exe 自己产出——
> 否则"AI 写完脚本测到通过"这句话里没有任何一环是可信的。

## 一、四条命令

```bat
curseen.exe --lua-check assets/lua/HoverDot/foo.lua        :: 能否加载 + schema 体检
curseen.exe --lua-trace assets/lua/HoverDot/foo.lua        :: 跑 N 帧，给 verdict
curseen.exe --lua-shot  assets/lua/MouseTrail/foo.lua      :: 离屏出 PNG
curseen.exe --mcp                                          :: 以 MCP 服务器运行（stdio）
```

**路径一律相对程序目录**（`curseen.exe` 所在目录）。绝对路径只接受落在程序目录内的，
`..` 一律拒绝——工具不该成为越界读写的口子（便携铁律）。

脚本路径还收**简写**：`HoverDot/foo`、`HoverDot/foo.lua`、`assets/lua/HoverDot/foo.lua`
三种写法等价（`assets/lua/` 与 `.lua` 都可以省）。这不是宽容，是防止"刚写完的文件
check 却说读不到"——`write_script` 返回的 `id` 就是简写，调用方自然会再喂回来。

| 旗标 | 用于 | 说明 |
|---|---|---|
| `--frames N` | trace / shot | 跑多少帧，默认 300（5 秒）。收敛判定另有预算，见下 |
| `--at N` | shot | 取第几帧，默认 = 输入区间的末帧 |
| `--screen 1920x1080` | trace / shot | 逻辑屏幕尺寸（`screen_w/h` 的来源） |
| `--mode hover\|trail\|click\|composite` | 全部 | 强制宿主；默认按所在目录推断 |
| `--input sweep\|idle-swing` | trace / shot | 合成输入轮廓，见 §三 |
| `--param key=value` | trace / shot | 覆盖一个参数，可重复；值按 JSON 解析，失败当字符串 |
| `--out 路径` | shot | 输出 PNG（相对程序目录），默认 `temp/lua_shot/<id>.png` |

退出码：`0` 通过 / `2` 用法错误 / `3` 有警告或判据不过 / `4` 加载或运行出错。
`verdict` 之外还有一个 **`judged`**：`false` = 这类脚本在本环境判不了（缺位图），
`pass` 不等于"验过"——别把这两件事混成一句"测试通过"。
（`--lua-check` 的"警告"是 3：能加载但有隐患——声明了没读的参数、默认值不在滑块网格上。）

**没有 `schema()` 不算错**（= 没有参数区，宿主退化成空默认值），返回里 `schema_present:false`；
只有"写了 `schema()` 但抛异常"才是错。运行期报错带**脚本自己的 `文件:行号`**
（`load_script` 用 `set_name("@<路径>")` 给 chunk 起了名），AI 拿到就能直接改那一行。

## 二、判据（trace 自己算，调用方不用解释数字）

| 判据 | 含义 | 不过就是 bug |
|---|---|---|
| `first_frame_draws` | 第 0 帧必须产出指令 | 宿主按"这一帧画不画"判活，首帧不画 = 实例当场判死 |
| `draws_while_moving` | 输入期间必须有产出 | 脚本没把 `cursor_x/y` 变成图元，或全被透明度/尺寸挡掉 |
| `settles` | 悬浮标/复合：输入停止后 `draw` 必须**不再返回正数** | 帧是"按需"的，永不到 0 = 静止态永久烧 CPU |
| `stops` | 点击效果：必须自行停笔 | 不停笔的实例永不释放（管理器按"还画不画"剔除） |
| `cmds_sane` | 单帧指令 ≤ 20000 | 生成循环写成 O(n²) 或没设上限 |
| `draws_at_all` | 整段至少一条指令 | 完全没画 |

**收敛预算**：`settles` 判的是"**永不到 0**"而不是"慢"。欠阻尼的弹簧链停止输入后还会真实
摆动 6~9 秒（阻尼比约 0.16），所以输入结束后额外给 1200 帧（20 秒）预算，一旦收敛立刻停。
用"300 帧内必须归零"去判，会把正确的脚本判红。

**拖尾不判 `settles`**：`spider_web`（蛛丝缠网）这类整体一直在变是合法的，只在 `notes` 里提示。

## 三、合成输入（`--input`）

| 轮廓 | 轨迹 | 适用 |
|---|---|---|
| `sweep`（默认） | 横向 S 曲线扫过画面（≤90 帧），然后停住 | 跟随类 / 轨迹类；`first_frame_draws` 只在这个轮廓下判 |
| `idle-swing` | 先静止 90 帧（1.5 s），再左右各摆三次（每段 4 帧），然后停住 | **静止触发**类，如「晃动定位」：它平时什么都不画，只有"静止够久 + 左右回摆"才弹环 |

没有第二种轮廓时，`HoverDot/locate` 会被判成"零指令"的假 fail——**判据自己先错，比没有判据更糟**。

## 四、脚本放哪（决定用户能不能在面板里选到）

| 目录 | 出现在 | 热重载 |
|---|---|---|
| `assets/lua/MouseTrail/` | 「拖尾效果」卡片的效果下拉 | ❌ |
| `assets/lua/HoverDot/` | 「悬浮标」卡片的类型下拉 | ❌ |
| `assets/lua/ClickFx/` | 「点击效果」各键的下拉 | ❌ |
| `assets/lua/scripts/`（含 `scripts/包名/`） | 「脚本」页的常驻卡片，**不进**上面三个下拉 | ✅ 每帧 stat |
| `assets/lua/KeySound/` | 「点击声音」卡片的音效脚本 | ❌ |

底层驱动是同一份（`mouse_script_works_wherever_the_file_is_dropped` 钉住"丢哪都一样"），
**差别只在 UI 入口与热重载**。所以：

- 效果要能被卡片选中 → 必须放对应特效目录；
- 整套程序（热键、开窗、复合） → 放 `scripts/`（也只有它会热重载）；
- 键音脚本吃的是 `run(ctx, params)`（按键触发），**不是**逐帧脚本，本工具会明确拒绝驱动它。

## 五、MCP

`curseen.exe --mcp` = 同一套能力包成 [MCP](https://modelcontextprotocol.io) 的 `tools`。
面向外部客户端的协议细节、工具签名与排查见随包 `assets/docs/MCP说明.md`（本节只记实现口径）：

| 工具 | 等价命令 |
|---|---|
| `lua_check` | `--lua-check` |
| `lua_trace` | `--lua-trace` |
| `lua_shot` | `--lua-shot`（**额外返回 PNG 图片块**，无文件系统权限的客户端也能"看见"） |
| `list_scripts` | 扫五个挂载点，返回 id / 宿主 / UI 入口 / 是否热重载 |
| `read_doc` | 读一份随包 `.md`（`assets/docs/…` / `assets/lua/API.md` 等），原文进文本块 |
| `write_script` | **没有 CLI 对应**：把 `code` 写到 `assets/lua/<挂载目录>/<名字>.lua`，默认不覆盖 |
| `delete_script` | **没有**：删除 = 移进 `temp/lua_tool_trash/`（不真删，可找回） |
| `set_active` | **没有**：启用/停用脚本并写参数（按挂载点改对 `config.json` 的键） |

后三个是**给没有文件系统的客户端补的笔**：它们没有 CLI 等价物是刻意的——会用命令行的
agent 直接写文件更直接，不该再学一套动词。

## 五之二、闭环（没有文件系统的客户端）

```
write_script(path, code)          → 落盘，返回 id / ui_entry / hot_reload
lua_check(path)                   → 语法 + schema 体检（warnings 为空才算干净）
lua_trace(path, frames)           → verdict=pass|fail（judged=false 表示这类脚本离屏判不了）
set_active(script, params)        → 改 config.json；用户重启一次后就在面板里
```

`set_active` 替调用方记着各挂载点的**键位差异**（悬浮标用裸 stem、拖尾是扁平命名空间
`"<stem>.<key>"`、点击落 `ripple_bindings.<键>`、`scripts/` 落 `lua_ext.composite_active`），
返回的 `changed` 列出真的改了哪些键——想核对就对比它，不用自己拼路径。

它有三条硬规矩，都是踩过的：
- **不覆盖已有脚本**（`overwrite=false`）：出厂脚本被同名写入而悄悄消失，事后极难发现。
- **不凭空造 `config.json`**：缺文件就报错让你先启动一次程序，而不是写一份"自以为的出厂值"。
- **只动该动的键**：在 JSON 值上按路径改，不经过 `Config` 反序列化——否则会丢掉本版本
  不认识的字段（未来版本写进去的键）。

只实现 `tools`：`prompts` / `resources` / `sampling` / `roots` 一律不碰（各家实现不一致，
其中 sampling 还会把主从关系搞反）。传输只有 **stdio**、换行分帧，不开端口。

配置示例（客户端各不相同，本质都是"起一个 stdio 服务端"）：

```json
{
  "mcpServers": {
    "curseen": {
      "command": "D:/Apps/CurSeen/curseen.exe",
      "args": ["--mcp"]
    }
  }
}
```

`isError` = CLI 退出码 `>= 3` **或** 返回体 `ok:false`（参数级失败在 CLI 里是退出码 2，
但对客户端同样是"这次调用没成功"，只回 `ok:false` 会被模型当成"跑通了但结果为空"）。
工具结果的第一个内容块永远是文本 JSON，与命令行打印的完全一致——**两条入口一份口径**，不许分叉。

## 六、文档怎么让 AI 找到（别让它猜）

AI 最容易浪费时间的两个问题不是"怎么写"，而是"**说明在哪**"和"**配置怎么让我改的生效**"。
所以文档位置是工具输出的一部分，不是 README 里的一句话：

- `curseen.exe --help` 末尾列出**全部随包文档**的路径与一句话说明；
- `--lua-check` 的返回带 `docs` 数组（`path` + `about`），并在 `hints` 里给出与本次脚本
  所在目录相关的那一份；
- MCP 的 `read_doc` 让**没有文件权限**的客户端也能读到原文（只收 `.md`：HTML 手册是给人
  用浏览器看的，90KB 标签塞进上下文纯属浪费 token）。

| 文档 | 回答什么 |
|---|---|
| `assets/docs/接入指南.md` | **外部 AI / 测试者第一站**：两种接法、八个工具、四步闭环、判据口径、安全边界 |
| `assets/docs/MCP说明.md` | MCP 专章：握手、工具签名、参数默认值口径、错误码、排查（给外部客户端，不重复实现细节） |
| `assets/docs/README.md` | 索引：哪份文档讲什么、AI 的阅读顺序 |
| `assets/docs/配置说明.md` | `config.json` 每个键怎么写 + 五个案例（**启用 / 调参前必读**） |
| `assets/lua/API.md` | Lua 接口权威表（与手册同源校验） |
| `assets/lua/HoverDot/README.md` | 悬浮标脚本的额外约定（续帧 / `hide_cursor` / 坐标系） |

这份路径表在代码里是常量 `lua_tool::DOCS`，由
`tests/docs_consistency.rs::test_lua_tool_docs_point_at_real_files` 钉住"文件真的存在、
且索引里确实提到了它"——**文档被挪走而工具还指着老路径**，会在 CI 就红，不会在用户那里变成 404。

## 七、已知限制

- **离屏 `params` 不合并 `schema()` 默认值**：`trace` / `shot` 只把显式 `--param` 交给脚本；
  真机侧也只有 `image` 角色的 `run` 模型（`scripting/ext_runner.rs::merged_params`，m17）
  会合并，击键音效与四个回调宿主仍吃裸配置（现行契约见 `assets/lua/API.md` §8）。脚本里
  `params.x or V` 的 V 必须与 schema 的 `default` 一致，否则离屏与真机两条路径行为不同
  （外部 AI 实测：改默认值三轮出图不变，传 `--param` 立刻变）。修它会整体改变出厂脚本的
  判据基线，属于需单独量化回归面的行为变更；当前口径写在随包《MCP说明.md》§5。
- **`set_active` 改完要重启**：配置是启动时读一次、之后只读内存；而且程序运行中打开面板
  会用内存里的配置**覆盖**文件。所以顺序必须是"退出程序 → 改 → 启动"，或者"改完立刻重启"。
- **只有 `scripts/` 热重载**：脚本内容改完立刻生效；特效目录改完要重开面板/重启。

- **只判"能不能跑"，不判"好不好看"**。出图是给人/多模态看的，别把"看着还行"当成判据。
- **图片类悬浮标**（`HoverDot/png`、以及任何 `ctx:image(...)` 的脚本）在离屏里**没有位图**：
  `uses_image` 为真时首帧 / 移动期 / 总产量三条判据全部跳过，返回 `judged: false`。
  `verdict=pass` 在这一档只代表"能加载、参数合法、不空转"——**画面必须真机验收**。
  （这条曾经是红的：出厂 `HoverDot/png` 被判 `draws_at_all` fail，等于工具指着一个没坏的
  脚本说它坏了，AI 会照着去改。）
- **没有真机环境**：DPI 换算、多屏负坐标、覆盖层合成都不在离屏路径里。离屏通过 ≠ 真机好看；
  真机验收仍需人眼（或后续的 IPC 通道）。
- **不动正在运行的实例**：本工具是"另一台无头引擎"，不连正在跑的 CurSeen。真机反复调参
  目前只能"改文件 → 重开面板/重启"；给三个特效目录加热重载是下一步，不是本工具的职责。
- **提权**：工具分支在 `main()` 最前面，早于提权与单实例仲裁，所以它永远普通权限、
  不会把正在运行的实例顶掉。

## 八、安全边界

`--mcp` / `--lua-*` 能让调用方**写 `.lua` 并让程序执行它**（脚本有 `ext:proc` / `ctx.fs`）——
`write_script` 让这件事**连文件系统权限都不需要**了。所以：

- 只走 stdio / 本机管道，不开端口、不做远程；
- 路径锁在程序目录内，只能写 `assets/lua/` 的五个挂载目录；
- 默认不覆盖已有脚本，删除只是移进 `temp/lua_tool_trash/`（都在程序目录内）；
- 这份能力等价于"在本机执行任意代码"。交给谁，等于让谁能在本机跑东西。
