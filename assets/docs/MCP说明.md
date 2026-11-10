# MCP 说明（`curseen.exe --mcp`）

这份文档讲**协议与工具细节**：怎么握手、八个工具各自收什么参数、返回里每个字段什么意思、
出错了怎么判。它面向要把 CurSeen 接进自己客户端的人或 AI。

> 只想快速接上并跑通闭环，先读 [`接入指南.md`](接入指南.md)（两种接法、四步闭环、硬规则）。
> 那份是总览，本页是它的 MCP 专章；两页不重复推演过程。

## 1. 服务器怎么起

```bat
curseen.exe --mcp
```

- **传输只有 stdio**：一行一条完整 JSON，回复也是一行一条、写完立刻 flush。不开端口、
  不监听、不鉴权（本机管道即边界）。
- **只实现 `tools`**：`prompts` / `resources` / `sampling` / `roots` 一律不提供。
  `initialize` 的 `capabilities` 里只有 `{"tools": {}}`，别去声明没有的能力。
- **stdout 是协议通道**：服务器上头一个字节都不多打；日志走文件。
- **所有路径相对 `curseen.exe` 所在目录**（不是当前工作目录）。从哪个 cwd 启动都一样；
  绝对路径只接受落在程序目录内的，`..` 一律拒绝。
- 它是一台**独立的无头引擎**：不连接、也不影响正在运行的 CurSeen；不开窗、不提权、不抢单实例。

### 客户端配置

各家客户端字段名不同，本质都是"起一个 stdio 服务端"：

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

`command` 必须写**绝对路径**（客户端不会替你切到程序目录）。接上后先读
`initialize` 返回的 `instructions`——那是给模型的速查版规则。

## 2. 握手

请求：

```json
{"jsonrpc":"2.0","id":1,"method":"initialize",
 "params":{"protocolVersion":"2025-06-18","capabilities":{},
           "clientInfo":{"name":"my-client","version":"1"}}}
```

回复要点：

| 字段 | 值 | 说明 |
|---|---|---|
| `protocolVersion` | 原样回显客户端的版本 | 客户端不报时用 `2024-11-05` |
| `capabilities` | `{"tools": {}}` | 只提供工具 |
| `serverInfo` | `curseen-lua` / 版本号 | 版本号跟发行版一致 |
| `instructions` | 规则全文 | 脚本放哪、判活铁律、文档路径，都在这里 |

之后 `tools/list` 取工具清单，`tools/call` 调用。三条协议纪律：

- 没有 `id` 的消息是**通知**，服务器不回包（回了反而让严格客户端报错）；
- `id` 原样回，数字与字符串都合法；
- 单条消息不合法只回一条错误包，**不会退出**（`-32700` 解析失败 / `-32601` 不支持的方法 /
  `-32602` 参数不对）。

## 3. 八个工具

| 工具 | 输入 | 返回里要看的字段 |
|---|---|---|
| `list_scripts` | 无 | `scripts[]`：id / 宿主 / UI 入口 / 是否热重载 |
| `read_doc` | `path`（只收 `.md`） | `text` 原文、`chars`；HTML 手册会被明确拒绝 |
| `write_script` | `path`、`code`、`overwrite`(默认 false) | `id`、`ui_entry`、`hot_reload`、`created` |
| `lua_check` | `path`、`mode`(可选) | `ok`、`warnings[]`、`schema[]`、`schema_present`、`docs[]` |
| `lua_trace` | `path`、`frames`、`mode`、`params`、`input` | `verdict`、`failed[]`、`judged`、`checks{}` |
| `lua_shot` | `path`、`frames`、`at`、`out`、`mode`、`input`、`params` | `out` + **PNG 图片块**（base64） |
| `set_active` | `script`、`enabled`、`button`、`params` | `changed[]`（真的改了哪些配置键）、`notes[]` |
| `delete_script` | `path` | `moved_to`：移到回收站的位置 |

路径参数收三种等价写法：`HoverDot/foo`、`HoverDot/foo.lua`、
`assets/lua/HoverDot/foo.lua`。`write_script` 返回的 `id` 就是简写，直接喂回 check / trace 即可。

`mode` 取 `hover` / `trail` / `click` / `composite`，默认按脚本所在目录推断，一般不用填。
`input` 取 `sweep`（默认，扫过画面后停住）或 `idle-swing`（先静止 1.5 秒再左右回摆，
给"静止够久才触发"的脚本用；没有它会把正确的脚本判成零指令）。

挂载目录只认五个：`MouseTrail/` `HoverDot/` `ClickFx/` `KeySound/` `scripts/`。
前三个各进对应特效下拉，`scripts/` 是「脚本」页常驻卡片且**唯一热重载**。

## 4. 四步闭环

```
write_script(path, code)
  → lua_check(path)          warnings 必须为空
  → lua_trace(path)          verdict 必须 pass
  → set_active(script, ...)  用户重启一次程序，就在面板里
```

一次真实运行的返回节选：

```
write_script: ok=true  id=HoverDot/foo  ui_entry=「悬浮标」卡片的类型下拉
lua_check   : ok=true  warnings=[]  schema_present=true
lua_trace   : verdict=pass  judged=true  failed=[]
set_active  : enabled=true  changed=['trail.hover_enabled','trail.hover_type',
                                     'trail.hover_params.foo.size']
```

`set_active` 替调用方记着各挂载点的键位差异：`hover_type` 用**裸 stem**、拖尾参数是
扁平命名空间 `"<stem>.<key>"`、点击落 `ripple_bindings.<键>`、`scripts/` 落
`lua_ext.composite_active`。逐键手改的口径见 [`配置说明.md`](配置说明.md)。

三条硬规矩：**不覆盖已有脚本**（`overwrite=false`，要改同名就显式传 true）；
**不凭空造 `config.json`**（缺文件就报错让你先启动一次程序）；
**只动该动的键**（返回 `changed[]` 可核对，用户配置里其它字段原样保留）。

`delete_script` **不真删**：脚本移到 `temp/lua_tool_trash/`，随时可找回。

## 5. ★ 参数口径：`schema()` 的 default 不会自动进 params

这是最容易让"离屏判据过了、画面却不对"的一条，务必看清。

- **`lua_trace` / `lua_shot`（以及命令行的 `--lua-trace` / `--lua-shot`）只把显式传入的
  `--param` / `params` 交给脚本**。`schema()` 里声明的 `default` **不会**被补进 `params`。
- **真机侧只有 `image` 角色的 `run` 模型**会合并默认值；击键音效与四个回调宿主
  （拖尾 / 悬浮标 / 点击效果 / 复合）拿到的仍是裸配置，见 `assets/lua/API.md` §8。
- 所以脚本里写 `params.ribbon_width or 6`、而 schema 里 `default = 20` 时：
  离屏与吃裸配置的宿主跑的一直是 **6**，会合并的那路跑的是 **20**。这会出现"把 schema 默认值从 6 改到 16，
  离屏出图却毫无变化"的怪事——因为改的根本没生效。

**写法要求**：每个 `params.<key> or <V>` 的兜底值 `V`，必须与 `schema()` 里该键的
`default` **逐项一致**。这样两条路径行为相同，谁合并都不影响结果。
要临时覆盖某个参数，用 `--param key=value`（命令行）或 `params: {"key": value}`（MCP），
它一定会生效，也能用来验证某个键到底走的是哪条路径。

> `lua_check` 的静态体检能报"声明了但脚本里没读"的参数，**不会**报"兜底值与 default 不一致"；
> 这一条目前要脚本作者自己对齐。出厂脚本都是按这个口径写的，可照抄。

## 6. 判据与错误口径

`lua_trace` 的五条判据由工具自己算，不需要调用方解释数字：

| 判据 | 不过意味着 |
|---|---|
| `first_frame_draws` | 第 0 帧没产出指令 → 宿主按"这一帧画不画"判活，实例当场判死 |
| `draws_while_moving` | 指针在动却一帧都没产出 |
| `settles` / `stops` | 悬浮标/复合：静止后仍在要帧（永久烧 CPU）；点击：不停笔（实例永不释放） |
| `cmds_sane` | 单帧指令数失控 |
| `draws_at_all` | 整段零指令 |

两个容易误读的返回：

- **`judged=false`**：这类脚本在离屏判不了（用 `ctx:image` 的图片类没有位图）。此时
  `verdict=pass` 只代表"能加载、参数合法、不空转"，**画面必须真机验收**。
- **明确拒绝**：`KeySound/` 脚本的契约是 `run(ctx, params)`（按键触发一次），不是逐帧绘制；
  工具直接报错说明，而不是给一个"零指令"的假 fail。

`tools/call` 的结果里，第一个内容块永远是文本 JSON，**与命令行打印的逐字相同**；
`lua_shot` 额外带一个 `image` 图片块。

| 信号 | 含义 |
|---|---|
| `isError` | CLI 退出码 `>= 3` **或** 返回体 `ok=false`。参数级失败在 CLI 里是退出码 2，但对客户端同样是"这次没成功" |
| 退出码 `0/2/3/4` | 通过 / 用法错误 / 判据不过或有警告 / 加载或运行出错 |
| `schema_present=false` | 脚本没写 `schema()` 是**合法**的（没有参数区）；写了却抛异常才是错 |
| 运行期报错 | 带脚本自己的 `文件:行号`，直接改那一行即可 |

## 7. 安全边界

`write_script` 让"落盘 Lua 脚本"不再需要文件系统权限，而 Lua 脚本本身能起进程、
读写文件（`ext:proc` / `ext:write_file`；`ctx.fs` 只读）。**把这套工具交给谁，等于让谁在本机执行任意代码。**

- 只走 stdio / 本机管道，不开端口、不做远程；
- 写入锁在程序目录内 `assets/lua/` 的五个挂载目录，`..` 与目录外绝对路径一律拒绝；
- 默认不覆盖已有脚本；删除只是移进 `temp/lua_tool_trash/`（都在程序目录内）；
- `read_doc` 只读 `.md`，且不许越出程序目录。

## 8. 排查

| 现象 | 先查 |
|---|---|
| 敲了命令没有任何输出 | 客户端没把 stdio 接上；`--mcp` 只吃 stdin 上的一行一条 JSON |
| `tools/call` 报"没有这个工具" | 名字拼写；用 `tools/list` 取当前清单，别凭记忆 |
| `write_script` 返回 `exists=true` | 同名文件已存在且没传 `overwrite=true`；换名字或显式覆盖 |
| `set_active` 报缺 `config.json` | 先启动一次程序让它生成出厂配置，再改 |
| 改了配置没反应 | 程序正在运行时会用内存配置覆盖文件；**先退出程序再改**，首次启用要重启一次 |
| 改了 schema 默认值，离屏出图没变化 | 见 §5：默认值不进 params，查脚本里的 `or` 兜底值 |
| `verdict=pass` 但画面不对 | 看 `judged`；离屏只判"能不能跑"，好不好看要真机看 |
| 参数在面板上拖了没效果 | 参数必须被脚本真的读（`lua_check` 会把没读的报成 warning） |
| 脚本在下拉里找不到 | 放错挂载目录；`list_scripts` 的 UI 入口会告诉你它该在哪 |

## 9. 文档地图

| 要看什么 | 读哪个 |
|---|---|
| 怎么接、四步闭环、硬规则总览 | `assets/docs/接入指南.md` |
| 本页：MCP 协议 / 八个工具 / 参数口径 / 排查 | `assets/docs/MCP说明.md` |
| `config.json` 每个键怎么填 + 案例 | `assets/docs/配置说明.md` |
| Lua 接口（签名、事件、绘制上下文） | `assets/lua/API.md` |
| 悬浮标脚本的额外约定（续帧 / `hide_cursor`） | `assets/lua/HoverDot/README.md` |
| 文档索引 | `assets/docs/README.md` |
