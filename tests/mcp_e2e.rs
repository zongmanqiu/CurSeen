//! MCP 端到端门禁：**真拉起发行版 exe** 走一遍 stdio 协议。
//!
//! 单测验证的是函数；这一条验证的是"客户端按文档接线时，进程真的会回话"：
//! 分帧、stdout 只有协议帧、错误码、工具清单、文档清单，以及超长帧之后服务仍在。
//!
//! 只调用只读工具（`tools/list` / `list_docs` / 缺文件的 `read_doc`），
//! 不写任何脚本或配置——门禁跑在开发机上，不能顺手动用户的安装目录。

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value as Json};

/// 一次会话：把所有请求写进 stdin 后关闭，再收集 stdout 的每一行。
///
/// 服务端在 EOF 时正常退出，所以不需要读超时；顺序也由管道保证。
fn session(requests: &[String]) -> Vec<Json> {
    let exe = env!("CARGO_BIN_EXE_curseen");
    let mut child = Command::new(exe)
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("应能拉起 curseen.exe --mcp");
    {
        let mut stdin = child.stdin.take().expect("stdin 管道");
        for line in requests {
            writeln!(stdin, "{line}").expect("写请求");
        }
    }
    let out = child.wait_with_output().expect("等待服务端退出");
    let text = String::from_utf8(out.stdout).expect("stdout 应是 UTF-8 协议帧");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout 出现非协议帧：{e}\n{l}"))
        })
        .collect()
}

fn frame(id: u64, method: &str, params: Json) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// 握手 → 清单 → 文档 → 错误码：一条客户端实际会走的路。
#[test]
fn handshake_list_and_docs_follow_the_protocol() {
    let replies = session(&[
        frame(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        frame(2, "tools/list", json!({})),
        frame(
            3,
            "tools/call",
            json!({ "name": "list_docs", "arguments": {} }),
        ),
        frame(4, "ping", json!({})),
        frame(5, "no/such/method", json!({})),
        // 通知（没有 id）不回包
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        // 缺文件的 read_doc：必须是"工具失败"，不是协议错误
        frame(
            6,
            "tools/call",
            json!({
                "name": "read_doc",
                "arguments": { "path": "assets/docs/definitely-missing.md" }
            }),
        ),
        frame(
            7,
            "tools/call",
            json!({ "name": "host_status", "arguments": {} }),
        ),
    ]);

    assert_eq!(
        replies.len(),
        7,
        "七条带 id 的请求应有七条回包：{replies:#?}"
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
    assert!(
        replies[0]["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.contains("lua_check") && s.contains("read_doc")),
        "instructions 必须给出闭环与文档入口"
    );

    let tools: Vec<&str> = replies[1]["result"]["tools"]
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
        assert!(tools.contains(&expect), "工具清单缺 {expect}：{tools:?}");
    }
    for tool in replies[1]["result"]["tools"].as_array().expect("数组") {
        assert!(
            tool["inputSchema"]["type"] == "object",
            "每个工具都要给 JSON Schema：{tool}"
        );
    }

    let docs_payload: Json = serde_json::from_str(
        replies[2]["result"]["content"][0]["text"]
            .as_str()
            .expect("list_docs 文本块"),
    )
    .expect("list_docs 结果是 JSON");
    let docs = docs_payload["docs"].as_array().expect("docs 是数组");
    assert!(docs.len() >= 10, "主题文档应已登记：{} 项", docs.len());
    let names: Vec<&str> = docs.iter().filter_map(|d| d["path"].as_str()).collect();
    for expect in [
        "assets/docs/鼠标特效.md",
        "assets/docs/虚拟键盘.md",
        "assets/docs/屏幕与捕获.md",
        "assets/docs/工具与小部件.md",
        "assets/docs/文件与目录.md",
        "assets/docs/脚本扩展与资源包.md",
    ] {
        assert!(names.contains(&expect), "docs 清单缺 {expect}：{names:?}");
    }

    assert_eq!(replies[3]["result"], json!({}), "ping 回空对象");
    assert_eq!(replies[4]["error"]["code"], -32601, "未知方法必须 -32601");
    let payload: Json = serde_json::from_str(
        replies[5]["result"]["content"][0]["text"]
            .as_str()
            .expect("文本块"),
    )
    .expect("工具结果是 JSON");
    assert_eq!(payload["ok"], false, "缺文件必须 ok=false");
    assert_eq!(replies[5]["result"]["isError"], true);
    let hs: Json = serde_json::from_str(
        replies[6]["result"]["content"][0]["text"]
            .as_str()
            .expect("host_status 文本块"),
    )
    .expect("host_status 结果是 JSON");
    assert_eq!(hs["ok"], true);
    assert!(hs["running"].is_boolean(), "必须明确报 running：{hs}");
}

/// 便捷命令：打印的配置块必须能直接粘（绝对路径 + `--mcp`），且不碰任何客户端配置。
#[test]
fn mcp_init_prints_a_pasteable_block() {
    let exe = env!("CARGO_BIN_EXE_curseen");
    let out = Command::new(exe)
        .arg("--mcp-init")
        .stdin(Stdio::null())
        .output()
        .expect("应能运行 --mcp-init");
    let text = String::from_utf8(out.stdout).expect("--mcp-init 输出应是 UTF-8");
    assert!(
        text.contains("\"mcpServers\""),
        "应打印 mcpServers 块：{text}"
    );
    assert!(text.contains("\"args\""), "应带 args：{text}");
    assert!(text.contains("--mcp"), "args 应是 --mcp：{text}");
    assert!(
        text.contains("curseen.exe"),
        "command 应是本机 exe 的绝对路径：{text}"
    );
}

/// 坏 JSON、超长帧、未来版本：一条都不能把服务打断，也不能污染帧边界。
#[test]
fn bad_frames_do_not_derail_the_session() {
    let huge = "x".repeat(8 * 1024 * 1024 + 64);
    let replies = session(&[
        "这不是 JSON".to_string(),
        huge,
        frame(1, "initialize", json!({ "protocolVersion": "2099-01-01" })),
        frame(
            2,
            "tools/call",
            json!({ "name": "no_such_tool", "arguments": {} }),
        ),
    ]);

    assert_eq!(
        replies.len(),
        4,
        "四条请求（含两条坏帧）各一条回包：{replies:#?}"
    );
    assert_eq!(replies[0]["error"]["code"], -32700, "坏 JSON 必须 -32700");
    assert_eq!(
        replies[1]["error"]["code"], -32700,
        "超长帧必须整帧丢弃并报 -32700"
    );
    assert_eq!(
        replies[2]["result"]["protocolVersion"], "2025-06-18",
        "未来版本要回退到我们支持的最新版，不能原样回显"
    );
    assert_eq!(replies[3]["error"]["code"], -32602, "不存在的工具要 -32602");
}
