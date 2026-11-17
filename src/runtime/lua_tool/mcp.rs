//! MCP 服务器（stdio）：把工具台的能力包成 `tools`，让**只会调工具的** AI 客户端
//! （Claude Desktop 那类，没有 shell / 文件权限）也能写脚本并测到通过。
//!
//! # 只实现 tools
//!
//! `prompts` / `resources` / `sampling` / `roots` 一律不碰：前两者各家实现不一致，
//! 后两者会把主从关系搞反（sampling 是让服务端反过来请求客户端调模型）。工具这一层
//! 是所有客户端的最小公共交集，多实现一个就可能踩到一个客户端只认一半的坑。
//!
//! # 传输
//!
//! 只用 **stdio**、换行分帧（MCP 的 stdio 传输就是这个）。不开端口——一旦监听就要鉴权、
//! 要防本机其它进程乱调、要写"端口被占"的支持文档，而收益只有"远程 agent 也能用"。
//!
//! # 协议纪律
//!
//! - 每一行必须是**完整的一条 JSON**，回复也必须一行一条、写完立刻 flush。
//! - **stdout 是协议通道**：这里一个字节都不能多打（日志走文件，不走这里）。
//! - 通知（没有 `id`）不回包；`id` 原样回（字符串与数字都合法）。
//! - 单条消息不合法只回一条错误包，不许退出——客户端会拼出多行的场景比想象中多。
//!
//! # 工具结果
//!
//! `tools/call` 的 `content` 里第一个永远是**文本块**（与 CLI 打印的 JSON 逐字相同），
//! `lua_shot` 额外带一个 **image 块**（PNG，base64）——这样没有文件系统的客户端也能"看见"
//! 效果。`isError` = CLI 退出码 `>=3` **或** 返回体 `ok:false`：参数级失败（缺 `path`、
//! 路径穿越）在 CLI 里是退出码 2（"用法错误"），但对 MCP 客户端就是"这次调用没成功"——
//! 只回一个 `ok:false` 而 `isError:false`，模型会当成"跑通了但结果为空"。

use std::io::{BufRead, Write};

use serde_json::{json, Value as Json};

use super::{Ctx, Flags, Out};

/// 与服务端版本无关的协议版本兜底：客户端不报时用它。
const DEFAULT_PROTOCOL: &str = "2024-11-05";

/// 协议版本协商表（新→旧）。客户端报的版本不在表里时回最新的一个——MCP 的规则是
/// 服务端只能答应自己实现过的版本，原样回显一个未来版本等于替客户端撒谎。
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// 单帧上限 8 MiB。超过就整帧丢弃并回一条 -32700：不设上限时一行就能把进程的内存
/// 吃光；只清缓冲不丢到行尾，剩下的半帧会被当成新帧，之后的帧边界全部错位。
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// 从 stdin 读一帧（到 `\n` 为止，去掉行尾 `\r`）。返回 false = EOF。
///
/// 用 `fill_buf` 分块读而不是 `read_line`：后者没有上限，超长帧会先分配再判断。
/// 超限时把这一帧剩下的字节一路吃到行尾，绝不把半帧留在管道里。
fn read_frame(stdin: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<Option<bool>> {
    buf.clear();
    let mut discarding = false;
    loop {
        let avail = stdin.fill_buf()?;
        if avail.is_empty() {
            return Ok(if buf.is_empty() && !discarding {
                None
            } else {
                Some(!discarding)
            });
        }
        match avail.iter().position(|&b| b == b'\n') {
            Some(i) => {
                if !discarding {
                    // 一整个块里就带行尾时也要先量长度：只在前面的"没找到换行"分支
                    // 判上限，超长行会在这里被整段收进缓冲——上限就形同虚设。
                    if buf.len() + i > MAX_FRAME {
                        discarding = true;
                        buf.clear();
                    } else {
                        buf.extend_from_slice(&avail[..i]);
                    }
                }
                stdin.consume(i + 1);
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                return Ok(Some(!discarding));
            }
            None => {
                let n = avail.len();
                if !discarding {
                    buf.extend_from_slice(avail);
                    if buf.len() > MAX_FRAME {
                        discarding = true;
                        buf.clear();
                    }
                }
                stdin.consume(n);
            }
        }
    }
}

/// 运行到 stdin 关闭为止。返回进程退出码（0）。
pub fn serve(ctx: &Ctx, out: &mut Out) -> i32 {
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut frame: Vec<u8> = Vec::new();
    loop {
        let ok = match read_frame(&mut lock, &mut frame) {
            Ok(Some(ok)) => ok,
            Ok(None) => return 0, // EOF：客户端收工
            Err(_) => return 0,   // stdin 不可读（直接双击运行等）：安静退出，别刷错误
        };
        if !ok {
            reply(out, &error(None, -32700, "帧超过 8 MiB 上限，已整帧丢弃"));
            continue;
        }
        let line = String::from_utf8_lossy(&frame);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Json = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                reply(out, &error(None, -32700, &format!("JSON 解析失败：{e}")));
                continue;
            }
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        // 通知：没有 id，不回包（回了反而会让严格客户端报错）。
        if id.is_none() {
            continue;
        }
        let id = id.unwrap_or(Json::Null);
        let result = match method {
            "initialize" => Ok(initialize(&req)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools_list(ctx)),
            "tools/call" => tools_call(ctx, &req),
            other => Err((-32601, format!("不支持的方法：{other}"))),
        };
        match result {
            Ok(v) => reply(out, &json!({ "jsonrpc": "2.0", "id": id, "result": v })),
            Err((code, msg)) => reply(out, &error(Some(id), code, &msg)),
        }
    }
}

fn initialize(req: &Json) -> Json {
    let asked = req
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PROTOCOL);
    // 只答应实现过的版本：客户端报了未来版本就回最新的一个，由它决定要不要断开。
    let asked = if SUPPORTED_PROTOCOLS.contains(&asked) {
        asked
    } else {
        SUPPORTED_PROTOCOLS[0]
    };
    json!({
        "protocolVersion": asked,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "curseen-lua",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// 给模型的固定说明。**这段文字就是"工具说明书"的一半**，比代码更容易决定成败：
/// 模型不知道"脚本该放哪、判据长什么样"，就会写出放在下拉里选不到的脚本。
const INSTRUCTIONS: &str = "\
CurSeen 的 Lua 脚本工具台。目标：脚本达到 verdict=pass，并且用户真的能在面板里选到它。

标准闭环（四步，别跳）：
  1) write_script   写到 assets/lua/<挂载目录>/<名字>.lua（小改用 patch_script）
  2) lua_check      语法 / 参数体检（参数声明了有没有被真读、默认值在不在滑块网格上）
  3) lua_trace      离屏跑 N 帧 → verdict=pass|fail。判据由工具算，不需要你解释数字。
  4) set_active     改 config.json 启用并写参数（配置要重启一次才完全生效）
  5) host_status    宿主在跑时可查：它有没有看见这次文件改动（seen_at / pending）

放哪个目录决定用户在哪能选到它（放错 = 用户根本找不到）：
  assets/lua/MouseTrail/  拖尾特效下拉（轨迹类：on_move + step + draw）
  assets/lua/HoverDot/    悬浮标类型下拉（跟随类：draw 每帧画一次）
  assets/lua/ClickFx/     点击效果下拉（on_press 触发一次，之后自行淡出停止）
  assets/lua/KeySound/    击键音效（按键触发一次：run(ctx, params)，不是逐帧绘制）
  assets/lua/scripts/     「脚本」页的常驻卡片，不进上面四个下拉
五个目录都支持热重载：特效/悬浮标/点击/复合脚本在文件变化后重建 VM；KeySound 在下一次按键生效。

铁律（trace 会自动判，不过就是不过）：
1. draw 第 0 帧必须产出指令——宿主按「这一帧画不画」判活，首帧不画 = 实例当场判死。
2. 悬浮标/复合脚本：指针停住后若还有动画（弹簧回摆、平滑追赶、淡出），draw 必须
   返回一个正数（毫秒，如 16）继续要帧；收敛后必须不再返回（否则静止态永久烧 CPU）。
3. 点击效果：必须自己停笔（返回前先判寿命），否则实例永不释放。
4. 参数只能在 schema() 里声明，并且在脚本里真的读（params.<key>），否则面板上拖动无效果。

第一次接入先读 assets/docs/接入指南.md：怎么连、工具清单、四步闭环、什么算通过。

读文档用 read_doc（只收 .md；不确定有哪些文档先调 list_docs）：
  assets/docs/接入指南.md        接入第一站：MCP / 命令行配置、工具表、闭环、判据口径
  assets/docs/MCP说明.md         MCP 协议 / 工具签名 / 参数默认值口径 / 排查
  assets/docs/Lua开发指南.md     脚本怎么写：结构 / schema 控件 / 生命周期 / 调试 / 发布
  assets/docs/README.md          索引：哪份文档回答什么、按问题查
  assets/docs/鼠标特效.md        拖尾 / 悬浮标 / 点击效果 / 点击声音 + 三个脚本目录契约
  assets/docs/虚拟键盘.md        击键回显 / 击键音效 / 键盘方案 JSON / 直播捕获
  assets/docs/屏幕与捕获.md      标注 / 放大 / 截图贴图 / GIF / 录屏 + 产物落点
  assets/docs/工具与小部件.md    便签 / 剪贴板历史 / 计时器（无脚本接口）
  assets/docs/文件与目录.md      程序版图 / 读写下界 / temp 与 data / 清理
  assets/docs/脚本扩展与资源包.md role / ext 能力 / pack.json / 在线包
  assets/docs/配置说明.md        config.json 每个键 + 案例（要手改配置时读）
  assets/lua/API.md              Lua 接口权威表
  assets/lua/HoverDot/README.md  悬浮标脚本额外约定（续帧 / hide_cursor / 坐标系）

注意：verdict=pass 但 judged=false 表示这类脚本在离屏判不了（图片类没有位图），
不等于画面验过，那类必须真机看。所有路径都相对程序目录。";

fn tools_list(_ctx: &Ctx) -> Json {
    let mode = json!({
        "type": "string",
        "enum": ["hover", "trail", "click", "composite"],
        "description": "强制宿主。默认按脚本所在目录推断（HoverDot→hover 等），一般不用填。"
    });
    let input = json!({
        "type": "string",
        "enum": ["sweep", "idle-swing"],
        "description": "合成输入轮廓。sweep（默认）= 扫过画面后停住；idle-swing = 先静止 1.5 秒再左右回摆，用于「静止一段时间才触发」的脚本（如晃动定位），否则它一条指令都不会画。"
    });
    json!({
        "tools": [
            {
                "name": "lua_check",
                "description": "检查一个 Lua 脚本能否在本程序里加载：返回语法/运行错误、schema() 的参数表、\
                                以及静态体检警告（声明了没读的参数、默认值不在滑块网格上、颜色格式…）。\
                                写脚本后先跑这个。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "相对程序目录的脚本路径，如 assets/lua/HoverDot/foo.lua" },
                        "mode": mode,
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "lua_trace",
                "description": "离屏跑 N 帧真实驱动（含合成鼠标轨迹），返回 verdict=pass/fail 与逐帧账本。\
                                判据由工具自己算：首帧是否产出、移动期是否有产出、静止后是否停止续帧、单帧指令数是否失控。\
                                这是唯一能抓「鼠标不动就冻住」这类行为 bug 的手段。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "frames": { "type": "number", "description": "跑多少帧，默认 300（=5 秒）" },
                        "mode": mode,
                        "params": {
                            "type": "object",
                            "description": "覆盖参数，键名同 schema() 的 key，如 {\"count\":10}",
                        },
                        "input": input,
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "lua_shot",
                "description": "把脚本第 N 帧离屏渲染成 PNG（深/浅四种底色）并**直接把图片返回**。\
                                用于看构图、对比度、指针可读性。图会写到 temp/lua_shot/ 下，同时作为图片块返回。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "frames": { "type": "number", "description": "总帧数（默认 300）" },
                        "at": { "type": "number", "description": "取第几帧，默认=最后一个指针还在动的帧" },
                        "mode": mode,
                        "input": input,
                        "params": { "type": "object" },
                        "out": { "type": "string", "description": "输出路径（相对程序目录），默认 temp/lua_shot/<id>.png" },
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "write_script",
                "description": "把脚本写到 assets/lua/<挂载目录>/<名字>.lua（overwrite=false 时不覆盖已有文件）。                                返回里带 ui_entry / hot_reload：告诉你用户在哪能看到它、要不要重开面板。                                写完必须接着跑 lua_check，**别跳过验证**。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "目标路径，如 assets/lua/HoverDot/foo.lua 或 HoverDot/foo.lua；                                            挂载目录只认 MouseTrail / HoverDot / ClickFx / KeySound / scripts。",
                        },
                        "code": { "type": "string", "description": "Lua 源码全文" },
                        "overwrite": {
                            "type": "boolean",
                            "description": "true 才允许覆盖同名文件（默认 false：拒绝覆盖别人/出厂脚本）",
                        },
                    },
                    "required": ["path", "code"],
                },
            },
            {
                "name": "delete_script",
                "description": "把一个脚本移进 temp/lua_tool_trash/（**不真删**，可找回）。用于清理测试脚本。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "同上：assets/lua/... 或 HoverDot/foo" }
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "patch_script",
                "description": "对已有脚本做小补丁（不重发全文）：replace / insert_after / delete 按顺序应用。\
                                每条 old_text 必须在当前文本里**唯一命中**，否则整批拒绝、文件不动；\
                                preview=true 只回变更摘要不落盘。写盘前把原文件备份到 temp/lua_tool_trash/。\
                                回执带 changed / lines_before / lines_after / first_changed_line / last_changed_line，\
                                改完只需重读变动的那几行。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "已有脚本的路径，写法同 write_script" },
                        "edits": {
                            "type": "array",
                            "description": "1-20 项，按顺序应用。op=replace 需要 new_text；insert_after 把 new_text 插到 old_text 之后；delete 删除 old_text。",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "op": { "type": "string", "enum": ["replace", "insert_after", "delete"] },
                                    "old_text": { "type": "string" },
                                    "new_text": { "type": "string" },
                                },
                                "required": ["op", "old_text"],
                            },
                        },
                        "preview": { "type": "boolean", "description": "true 只回变更摘要，不写盘（默认 false）" },
                    },
                    "required": ["path", "edits"],
                },
            },
            {
                "name": "list_docs",
                "description": "列出随包说明文档（path + 一句话说明）。不确定该读哪份时先调它，再用 read_doc 读原文。",
                "inputSchema": { "type": "object", "properties": {} },
            },
            {
                "name": "set_active",
                "description": "启用/停用一个脚本，并可顺带写入它的参数（改 config.json）。                                不必记各种参数表的口径：悬浮标用裸 stem、拖尾是扁平命名空间、                                点击落在 ripple_bindings.<键>、scripts/ 落在 lua_ext.composite_active——                                这个工具按挂载点替你写对。返回 changed 列出真的改了哪些键。                                五个挂载都支持热重载：特效/悬浮标/点击脚本改文件后重建实例，KeySound 在下一次按键生效。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "脚本路径或 id，如 HoverDot/foo" },
                        "enabled": { "type": "boolean", "description": "默认 true" },
                        "button": {
                            "type": "string",
                            "enum": ["left", "right", "middle", "x1", "x2"],
                            "description": "仅点击效果用：设为哪个键，默认 left",
                        },
                        "params": {
                            "type": "object",
                            "description": "要写入的参数（键名同 schema() 的 key），如 {\"size\":18}",
                        },
                    },
                    "required": ["script"],
                },
            },
            {
                "name": "read_doc",
                "description": "读一份随包说明文档（只收 .md）。写脚本前先读 API.md；                                要改配置（启用脚本 / 设参数）先读 assets/docs/配置说明.md——                                那里有每个键的口径与完整案例，以及\"先退出程序再改\"这类硬规则。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "相对程序目录的路径，取值见 lua_check 返回的 docs 字段。                                            常用：assets/docs/README.md（索引）、assets/docs/配置说明.md、                                            assets/lua/API.md、assets/lua/HoverDot/README.md",
                        }
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "read_script",
                "description": "读一个脚本的原文（只读）。路径写法同 patch_script：HoverDot/foo、HoverDot/foo.lua、assets/lua/... 三者等价；只认五个挂载目录。返回全文、行数、字节数与宿主入口。改脚本前先用它核对 old_text。",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "相对程序目录的脚本路径，如 HoverDot/foo" },
                    },
                    "required": ["path"],
                },
            },
            {
                "name": "host_status",
                "description": "读宿主（已启动的 CurSeen GUI）状态：在不在跑、pid、上次资产重扫时间、各挂载脚本数；并把磁盘最新修改时间与宿主看见的时间对比。写完脚本想确认「宿主有没有看见」时调它。宿主没开不是错误，返回 running=false。",
                "inputSchema": { "type": "object", "properties": {} },
            },
            {
                "name": "list_scripts",
                "description": "列出程序里现有的全部 Lua 脚本（id、所在宿主、UI 入口、是否热重载）。\
                                写新脚本前先看一眼同类脚本叫什么、放哪里。",
                "inputSchema": { "type": "object", "properties": {} },
            },
        ]
    })
}

fn tools_call(ctx: &Ctx, req: &Json) -> Result<Json, (i32, String)> {
    let params = req
        .get("params")
        .ok_or((-32602, "缺少 params".to_string()))?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "缺少工具名".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // 工具参数 → CLI 旗标：**同一份解析器**，避免 MCP 与命令行两条口径分叉。
    let mut flags = Vec::new();
    if let Some(m) = args.get("mode").and_then(|v| v.as_str()) {
        flags.push("--mode".to_string());
        flags.push(m.to_string());
    }
    if let Some(n) = args.get("frames").and_then(|v| v.as_u64()) {
        flags.push("--frames".to_string());
        flags.push(n.to_string());
    }
    if let Some(n) = args.get("at").and_then(|v| v.as_u64()) {
        flags.push("--at".to_string());
        flags.push(n.to_string());
    }
    if let Some(i) = args.get("input").and_then(|v| v.as_str()) {
        flags.push("--input".to_string());
        flags.push(i.to_string());
    }
    if let Some(o) = args.get("out").and_then(|v| v.as_str()) {
        flags.push("--out".to_string());
        flags.push(o.to_string());
    }
    if let Some(p) = args.get("params").and_then(|v| v.as_object()) {
        for (k, v) in p {
            flags.push("--param".to_string());
            flags.push(format!("{k}={v}"));
        }
    }
    let parsed = Flags::parse(&flags).map_err(|e| (-32602, e))?;

    let (code, mut payload, image) = match name {
        "list_scripts" => (
            0,
            json!({ "ok": true, "scripts": super::list_scripts(ctx) }),
            None,
        ),
        "list_docs" => (0, json!({ "ok": true, "docs": super::docs_json() }), None),
        "host_status" => {
            let (code, v) = host_status(ctx);
            (code, v, None)
        }
        "lua_check" | "lua_trace" | "lua_shot" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, format!("{name} 需要 path")))?;
            // 复用 CLI 的实现（路径校验、判据、出图全在那边），不另写一份口径。
            let (code, v) = match name {
                "lua_shot" => super::cmd_shot(ctx, path, &parsed),
                "lua_trace" => super::cmd_trace(ctx, path, &parsed),
                _ => super::cmd_check(ctx, path, &parsed),
            };
            let img = if name == "lua_shot" && code == 0 {
                v.get("out")
                    .and_then(|o| o.as_str())
                    .and_then(|rel| read_png_b64(ctx, rel))
            } else {
                None
            };
            (code, v, img)
        }
        "write_script" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "write_script 需要 path".to_string()))?;
            let code = args
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "write_script 需要 code".to_string()))?;
            let overwrite = args
                .get("overwrite")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (code, v) = super::write::write_script(ctx, path, code, overwrite);
            (code, v, None)
        }
        "patch_script" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "patch_script 需要 path".to_string()))?;
            let edits = args
                .get("edits")
                .ok_or((-32602, "patch_script 需要 edits".to_string()))?;
            let preview = args
                .get("preview")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (code, v) = super::write::patch_script(ctx, path, edits, preview);
            (code, v, None)
        }
        "delete_script" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "delete_script 需要 path".to_string()))?;
            let (code, v) = super::write::delete_script(ctx, path);
            (code, v, None)
        }
        "set_active" => {
            let script = args
                .get("script")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "set_active 需要 script".to_string()))?;
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let button = args.get("button").and_then(|v| v.as_str());
            let params = args.get("params");
            let (code, v) = super::write::set_active(ctx, script, enabled, button, params);
            (code, v, None)
        }
        "read_script" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "read_script 需要 path".to_string()))?;
            let (code, v) = super::write::read_script(ctx, path);
            (code, v, None)
        }
        "read_doc" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "read_doc 需要 path".to_string()))?;
            let (code, v) = read_doc(ctx, path);
            (code, v, None)
        }
        other => return Err((-32602, format!("没有这个工具：{other}"))),
    };

    // 失败时直接把下一步写给调用方：模型省一轮猜"接下来该干嘛"，工具的后台说明
    // 也不必散落在每段 description 里。已有 `next` 的（如 write_script）不覆盖。
    if let Some(hint) = next_hint(name, code, &payload) {
        if let Some(obj) = payload.as_object_mut() {
            obj.entry("next")
                .or_insert_with(|| Json::String(hint.to_string()));
        }
    }

    let mut content = vec![json!({
        "type": "text",
        "text": serde_json::to_string_pretty(&payload).unwrap_or_default(),
    })];
    if let Some(b64) = image {
        content.push(json!({
            "type": "image",
            "data": b64,
            "mimeType": "image/png",
        }));
    }
    let failed = code >= 3 || payload.get("ok").and_then(|v| v.as_bool()) == Some(false);
    Ok(json!({
        "content": content,
        "isError": failed,
    }))
}

/// 失败/成功后给调用方的下一步。`None` = 这个工具不需要额外指路。
fn next_hint(name: &str, code: i32, payload: &Json) -> Option<&'static str> {
    let ok = code == 0 && payload.get("ok").and_then(|v| v.as_bool()) != Some(false);
    match (name, ok) {
        ("lua_check", true) => {
            Some("语法与参数体检通过后，用 lua_trace 拿 verdict；改完再跑 lua_check。")
        }
        ("lua_check", false) => Some("按 error / warnings 修脚本，然后重跑 lua_check。"),
        ("lua_trace", true) => Some("看构图用 lua_shot；确认行为后用 set_active 启用。"),
        ("lua_trace", false) => Some("按 failed[] 与逐帧账本修脚本，然后重跑 lua_trace。"),
        ("lua_shot", true) => Some("图只回答构图；行为判据仍以 lua_trace 为准。"),
        ("patch_script", true) => {
            Some("接着跑 lua_check；本次若 preview=true，改完记得以 preview=false 落盘。")
        }
        ("read_script", true) => {
            Some("改一两行用 patch_script：old_text 从 text 里原样复制，必须唯一命中。")
        }
        ("read_script", false) => Some("先调 list_scripts 看现有脚本；新建用 write_script。"),
        ("patch_script", false) => {
            Some("old_text 必须唯一命中；先用 read_script / read_doc 核对原文，整批未写入。")
        }
        ("list_docs", true) => Some("用 read_doc 读取 path 指向的原文。"),
        ("read_doc", false) => Some("先调 list_docs 看有哪些文档；路径写相对程序目录的写法。"),
        ("set_active", true) => {
            Some("文件改动已热重载；配置项要用户重启一次才完全生效。可调 host_status 确认宿主看见了改动。")
        }
        ("host_status", _) => {
            Some("running=false 就让用户先启动 CurSeen；pending_ms>0 时等一秒再查一次。")
        }
        _ => None,
    }
}

/// 宿主状态：状态文件 + pid 存活 + 现场扫盘（磁盘比宿主新 = 可能还没被看见）。
fn host_status(ctx: &Ctx) -> (i32, Json) {
    let lua = ctx.app.join("assets").join("lua");
    let disk = json!({
        "mounts": crate::runtime::host_state::count_scripts(&lua),
        "latest_mtime_ms": crate::runtime::host_state::latest_mtime_ms(&lua),
    });
    let state_file = crate::runtime::host_state::REL;
    let Some(state) = crate::runtime::host_state::read(&ctx.app) else {
        return (
            0,
            json!({
                "ok": true,
                "running": false,
                "state_file": state_file,
                "disk": disk,
                "hint": "宿主没有启动过（或状态文件被清理）。先让用户启动 curseen.exe；热重载/面板状态都依赖它。",
            }),
        );
    };
    let pid = state.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let running = crate::platform::process::pid_alive(pid);
    let seen_at = state
        .get("last_assets_change_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let latest = disk["latest_mtime_ms"].as_u64().unwrap_or(0);
    let pending_ms = latest.saturating_sub(seen_at);
    let hint = if !running {
        "状态文件在，但 pid 已退出：用户关掉了 CurSeen。启动后再看。"
    } else if pending_ms > 1500 {
        "磁盘上有比宿主上次重扫更新的脚本：等一秒再查；仍不消失说明目录监听没在工作，让用户重开面板或重启。"
    } else {
        "宿主在跑，且已看见最近一次改动。"
    };
    (
        0,
        json!({
            "ok": true,
            "running": running,
            "state_file": state_file,
            "state": state,
            "disk": disk,
            "pending_ms": pending_ms,
            "hint": hint,
        }),
    )
}

///
/// 只收 `.md`：`操作手册.html` 是给人用浏览器看的（操作与快捷键），标签塞进上下文
/// 纯属浪费 token；AI 需要的规则都在同目录的 `.md` 里。
fn read_doc(ctx: &Ctx, path: &str) -> (i32, Json) {
    let full = match ctx.resolve(path) {
        Ok(p) => p,
        Err(e) => return (2, json!({ "ok": false, "error": e })),
    };
    let shown = ctx.display(&full);
    if full.extension().and_then(|e| e.to_str()) != Some("md") {
        return (
            2,
            json!({
                "ok": false,
                "path": shown,
                "error": "read_doc 只读 .md。操作手册.html 是给人用浏览器看的（操作与快捷键）；接口规则见 assets/lua/API.md 与 assets/docs/。",
            }),
        );
    }
    match std::fs::read_to_string(&full) {
        Ok(text) => (
            0,
            json!({
                "ok": true,
                "path": shown,
                "chars": text.chars().count(),
                "text": text,
            }),
        ),
        Err(e) => (
            2,
            json!({ "ok": false, "path": shown, "error": format!("读文件失败：{e}") }),
        ),
    }
}

/// 把刚写的 PNG 读出来做 base64（图片块要求 base64；不引第三方依赖，自己编码）。
fn read_png_b64(ctx: &Ctx, rel: &str) -> Option<String> {
    let p = ctx.resolve(rel).ok()?;
    std::fs::read(p).ok().map(|b| base64(&b))
}

/// 标准 base64（带 padding）。只为 MCP 的 image 块存在，不值得为它引一个 crate。
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn error(id: Option<Json>, code: i32, msg: &str) -> Json {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Json::Null),
        "error": { "code": code, "message": msg },
    })
}

fn reply(out: &mut Out, v: &Json) {
    let line = serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string());
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// base64 必须是标准字母表 + 正确的 padding：错了客户端解不出图，
    /// 而"图没出来"在 MCP 里表现为"工具返回了但看不见"，极难归因。
    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// `initialize` 要把客户端的协议版本原样回过去（严格客户端会校验），
    /// 并声明只提供 tools 能力。
    #[test]
    fn initialize_echoes_protocol_and_declares_tools_only() {
        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18" }
        });
        let r = initialize(&req);
        assert_eq!(r["protocolVersion"], "2025-06-18");
        assert!(r["capabilities"].get("tools").is_some());
        assert!(
            r["capabilities"].get("resources").is_none(),
            "只提供 tools，别声明没实现的能力"
        );
    }

    /// 客户端报了实现不了的协议版本时，回的是**我们支持的最新版**，不是原样回显——
    /// 答应一个没实现的版本等于替客户端撒谎，严格客户端会按新语义解析我们的帧。
    #[test]
    fn initialize_falls_back_to_a_supported_protocol() {
        let future = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2099-01-01" }
        });
        assert_eq!(
            initialize(&future)["protocolVersion"],
            SUPPORTED_PROTOCOLS[0]
        );
        let old = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        });
        assert_eq!(initialize(&old)["protocolVersion"], "2024-11-05");
    }

    /// 超长帧必须整帧丢弃：只清缓冲不吞到行尾，剩下的半帧会被当成新帧，之后的帧边界
    /// 全部错位（AHKDeck 的 mcp 端点踩过同一个坑）。
    #[test]
    fn oversized_frame_is_dropped_whole() {
        use std::io::Cursor;
        let big = format!("{}\n{{\"ok\":1}}\n", "x".repeat(MAX_FRAME + 16));
        let mut r = Cursor::new(big.into_bytes());
        let mut buf = Vec::new();
        assert_eq!(read_frame(&mut r, &mut buf).expect("读第一帧"), Some(false));
        assert!(buf.is_empty(), "超限帧不该留下半截内容");
        assert_eq!(read_frame(&mut r, &mut buf).expect("读第二帧"), Some(true));
        assert_eq!(buf, br#"{"ok":1}"#);
    }

    /// 工具名必须与 `tools_call` 的分支一一对应（改名后忘了改一边 = 调用永远报"没有这个工具"）。
    #[test]
    fn tool_names_line_up_with_dispatch() {
        let ctx = super::super::Ctx::from_app_dir(std::env::temp_dir());
        let list = tools_list(&ctx);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .expect("tools 是数组")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for expect in [
            "lua_check",
            "lua_trace",
            "lua_shot",
            "write_script",
            "patch_script",
            "delete_script",
            "set_active",
            "read_doc",
            "list_docs",
            "list_scripts",
            "read_script",
            "host_status",
        ] {
            assert!(names.contains(&expect), "缺少工具 {expect}：{names:?}");
        }
    }

    /// 宿主状态：没启动过的程序目录必须如实返回 `running:false`，而不是报错或假装在线。
    #[test]
    fn host_status_is_honest_when_no_host_has_run() {
        let app = fake_app("curseen_mcp_host_status_off");
        let ctx = Ctx::from_app_dir(app.clone());
        let (code, v) = host_status(&ctx);
        assert_eq!(code, 0);
        assert_eq!(v["ok"], true);
        assert_eq!(v["running"], false);
        assert_eq!(v["state_file"], "temp/lua_tool_state.json");
        let _ = std::fs::remove_dir_all(&app);
    }

    /// 同一进程写一份状态文件 ⇒ 读到自己的 pid，`running:true`。
    #[test]
    fn host_status_reads_a_live_state_file() {
        let app = fake_app("curseen_mcp_host_status_on");
        std::fs::create_dir_all(app.join("temp")).unwrap();
        std::fs::write(
            app.join("temp/lua_tool_state.json"),
            json!({
                "schema": 1,
                "version": "test",
                "pid": std::process::id(),
                "started_at_ms": 1,
                "updated_at_ms": 2,
                "asset_revision": 1,
                "last_assets_change_ms": 2,
                "panel_open": false,
                "hot_reload": ["MouseTrail"],
            })
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(app.join("assets/lua/HoverDot")).unwrap();
        std::fs::write(app.join("assets/lua/HoverDot/x.lua"), "x=1").unwrap();
        let ctx = Ctx::from_app_dir(app.clone());
        let (code, v) = host_status(&ctx);
        assert_eq!(code, 0);
        assert_eq!(v["running"], true, "自己的 pid 必须判为存活：{v}");
        assert_eq!(v["disk"]["mounts"]["HoverDot"], 1);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// `next` 提示里点名的工具必须真的存在：这次就是它让 AI 去用不存在的 `read_script`。
    #[test]
    fn every_tool_named_in_next_hint_exists() {
        let ctx = super::super::Ctx::from_app_dir(std::env::temp_dir());
        let list = tools_list(&ctx);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .expect("tools 是数组")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        let src = include_str!("mcp.rs");
        let start = src.find("fn next_hint").expect("应有 next_hint");
        let end = src[start + 1..]
            .find("\n/// ")
            .map(|i| start + 1 + i)
            .unwrap_or(src.len());
        let mut hinted = Vec::new();
        for line in src[start..end].lines() {
            let Some(rest) = line.trim_start().strip_prefix("(\"") else {
                continue;
            };
            let Some(q) = rest.find('"') else { continue };
            let name = &rest[..q];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                hinted.push(name.to_string());
            }
        }
        assert!(!hinted.is_empty(), "没抽到任何提示工具名，门禁自己失明了");
        for name in hinted {
            assert!(
                names.contains(&name.as_str()),
                "next 提示点名了不存在的工具 `{name}`：{names:?}"
            );
        }
    }

    /// 临时搭一个"程序目录"（只有 assets/docs 一份 md），减少对真实发行目录的依赖。
    fn fake_app(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("assets/docs")).expect("建目录");
        std::fs::write(p.join("assets/docs/a.md"), "# 标题\n\n正文\n").expect("写文档");
        std::fs::write(p.join("assets/docs/b.html"), "<html></html>").expect("写 html");
        p
    }

    /// 文档能给 AI 读成原文：这是"只会调工具的客户端"唯一能拿到说明的通道。
    #[test]
    fn read_doc_returns_markdown() {
        let app = fake_app("curseen_mcp_doc");
        let ctx = Ctx::from_app_dir(app.clone());
        let (code, v) = read_doc(&ctx, "assets/docs/a.md");
        assert_eq!(code, 0, "读 md 应当成功：{v}");
        assert_eq!(v["text"], "# 标题\n\n正文\n");
        assert_eq!(v["ok"], true);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// 只收 `.md`：HTML 手册有 90KB 标签，塞进上下文纯属浪费 token。
    /// 判据要看**错误码非 0 + 有可读的 error**，不是"返回了空 text"。
    #[test]
    fn read_doc_refuses_html_with_a_reason() {
        let app = fake_app("curseen_mcp_doc_html");
        let ctx = Ctx::from_app_dir(app.clone());
        let (code, v) = read_doc(&ctx, "assets/docs/b.html");
        assert_ne!(code, 0, "html 必须拒绝");
        let err = v["error"].as_str().unwrap_or_default();
        assert!(err.contains(".md"), "报错要说清只收 .md，实际：{err}");
        let _ = std::fs::remove_dir_all(&app);
    }

    /// 穿越与缺失都要有错：静默返回空串会让 AI 以为"这份文档没内容"。
    #[test]
    fn read_doc_rejects_traversal_and_missing() {
        let app = fake_app("curseen_mcp_doc_bad");
        let ctx = Ctx::from_app_dir(app.clone());
        let (c1, v1) = read_doc(&ctx, "../../windows/win.ini");
        assert_ne!(c1, 0);
        assert!(v1["error"].as_str().unwrap_or_default().contains(".."));
        let (c2, v2) = read_doc(&ctx, "assets/docs/nope.md");
        assert_ne!(c2, 0);
        assert!(v2["error"].as_str().is_some_and(|e| !e.is_empty()));
        let _ = std::fs::remove_dir_all(&app);
    }
}
