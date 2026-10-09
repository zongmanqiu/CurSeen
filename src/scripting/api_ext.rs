//! 暴露给 Lua 扩展脚本的 API（`assets/lua/*.lua` 里的 `run` 用）。
//!
//! 通过全局 userdata `ext` 调用，冒号语法：`ext:get_image()` / `ext:result(text)` /
//! `ext:http(method, url, headers, body)` / `ext:log(msg)`。
//!
//! # 上下文（贴图动作）
//!
//! 当脚本由贴图右键「图片操作」触发时，执行器（[`super::ext_runner`]）会在 `run` 前把
//! 当前贴图写进线程局部 [`EXT_CTX`]；`ext:get_image()` 据此返回贴图。在脚本页里跑
//! （没有贴图）时返回 `nil`。与 `api_draw` 的 TARGET 一样用"线程局部 + set/clear"，
//! 单线程下无生命周期问题。

use std::cell::RefCell;
use std::time::Duration;

use mlua::{Lua, UserData, UserDataMethods};

use crate::core::image::RgbaBitmap;

/// 当前动作上下文（贴图动作专用）：本线程正在跑的扩展脚本能拿到的东西。
#[derive(Clone)]
pub struct ExtCtx {
    /// 触发当前脚本的那张贴图（贴图右键全新克隆，脚本尽可持有）。
    pub image: RgbaBitmap,
    /// 结果框/弹窗的 owner 窗口句柄。
    pub hwnd: usize,
}

thread_local! {
    static EXT_CTX: RefCell<Option<ExtCtx>> = const { RefCell::new(None) };
}

/// 在调用扩展脚本的 `run` 前设置当前动作上下文；跑完务必配对调用 [`clear_ext_context`]。
pub fn set_ext_context(ctx: ExtCtx) {
    EXT_CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

/// 清空动作上下文（`run` 返回后调用，避免上下文泄漏到不该用它的脚本）。
pub fn clear_ext_context() {
    EXT_CTX.with(|c| *c.borrow_mut() = None);
}

/// 扩展 API 门面 userdata（`ext`）。无状态；所有状态走线程局部 [`EXT_CTX`]。
pub struct ExtApi;

impl UserData for ExtApi {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // ext:get_image() -> ExtImage | nil：当前贴图（贴图动作里才有）。
        methods.add_method("get_image", |lua, _this, ()| {
            let img = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.image.clone()));
            match img {
                Some(img) => lua
                    .create_userdata(ExtImage { img })
                    .map(mlua::Value::UserData),
                None => Ok(mlua::Value::Nil),
            }
        });
        // ext:result(text) -> 弹可选中复制文本框（不卡，自带消息泵）。
        // ★ `add_method`（不是 `add_function`）：脚本里一律 `ext:result(...)`（冒号）调用，
        //   冒号会把 `self`(=ext userdata) 当第一实参塞进闭包；用 `add_method` 让 `_this`
        //   吃掉它，否则 `text` 收到的是 userdata → "error converting Lua userdata to String"
        //   （2026-09-29 实测：这会导致整个 AI 对话脚本每步都报错、功能全废）。
        //   闭包体再套 `catch_unwind`：后台线程里弹原生框一旦真panic（桌面特定条件），
        //   转成 Lua error 而非 abort 掉整个进程。
        methods.add_method("result", |_lua, _this, text: String| {
            let hwnd = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::platform::win_util::text_result_box(hwnd as isize, "扩展脚本结果", &text);
            }));
            if r.is_err() {
                return Err(mlua::Error::external("结果框异常退出（已避免程序崩溃）"));
            }
            Ok(())
        });
        // ext:log(msg) -> 走 Rust 日志，方便排障。
        methods.add_method("log", |_lua, _this, msg: String| {
            log::info!("[ext] {msg}");
            Ok(())
        });
        // ext:input(title, default) -> string | nil：弹一个可编辑的输入框（自带消息泵，
        // 与 ext:result 同一条线程语义——在工作线程阻塞弹框、UI 零等待）。确定 = 返回
        // 输入文本；取消 / Esc / 关窗 = 返回 nil（脚本据此退出循环）。
        methods.add_method(
            "input",
            |_lua, _this, (title, default): (String, Option<String>)| {
                let hwnd = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0));
                // 套 `catch_unwind`：后台线程弹原生输入框一旦真 panic（桌面特定条件），转成
                // Lua error 而非 abort 掉整个进程（2026-09-29「脚本热键秒退」兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::platform::win_util::text_input_box(
                        hwnd as isize,
                        &title,
                        default.as_deref().unwrap_or(""),
                    )
                }));
                match r {
                    Ok(v) => Ok(v),
                    Err(_) => Err(mlua::Error::external("输入框异常退出（已避免程序崩溃）")),
                }
            },
        );
        // ext:json_decode(text) -> table | nil：JSON 文本 → Lua 表（数组 → 1 起始序列表）。
        // 解析失败 / 非法 JSON → nil（脚本自己兜底，不抛错）。
        methods.add_method(
            "json_decode",
            |lua, _this, text: String| match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => json_to_lua(lua, &v),
                Err(_) => Ok(mlua::Value::Nil),
            },
        );
        // ext:json_encode(value) -> string：Lua 表 / 标量 → JSON 文本。数字、布尔、字符串
        // 直转；表按"1..n 连续整数键 = 数组、其余 = 对象"分流；nil / 函数等不支持的值 → null。
        methods.add_method("json_encode", |lua, _this, v: mlua::Value| {
            let s = serde_json::to_string(&lua_to_json(&v)).unwrap_or_default();
            Ok(mlua::Value::String(lua.create_string(s.as_bytes())?))
        });
        // ext:http(method, url, headers, body) -> { status=int, body=string }（阻塞 HTTP）。
        methods.add_method(
            "http",
            |lua, _this, (method, url, headers, body): (String, String, mlua::Table, Option<String>)| {
                let mut hdrs: Vec<(String, String)> = Vec::new();
                for (k, v) in headers.pairs().flatten() {
                    if let (mlua::Value::String(k), mlua::Value::String(v)) = (k, v) {
                        hdrs.push((k.to_string_lossy(), v.to_string_lossy()));
                    }
                }
                // 套 `catch_unwind`：HTTP 走阻塞网络 + ureq，任何意外都转成
                // status=0 的错误响应，脚本据此显示失败而不是让进程 abort（2026-09-29 兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    http_blocking(&method, &url, &hdrs, body.as_deref())
                }));
                let (status, body_s) = match r {
                    Ok(v) => v,
                    Err(_) => (0, "请求线程异常（已避免程序崩溃）".to_string()),
                };
                let out = lua.create_table()?;
                out.set("status", status)?;
                out.set("body", body_s)?;
                Ok(out)
            },
        );
        // ext:chat_window(url, model, key, sys)：Pecia 式原生 AI 对话窗（顶部对话区 /
        // 中部「清除・发送」/ 底部输入框；Ctrl+Enter 发送、Esc 关窗、等待时标题栏读秒）。
        // 阻塞至关窗（与 ext:result / ext:input 同一条线程语义）；HTTP 在窗口的 worker
        // 线程里跑，界面全程不卡。多轮上下文在窗口内自持，关窗即弃。
        methods.add_method(
            "chat_window",
            |_lua, _this, (url, model, key, sys): (String, String, String, String)| {
                let hwnd = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0));
                // 套 `catch_unwind`：后台线程弹原生窗一旦真 panic（桌面特定条件），
                // 转成 Lua error 而非 abort 掉整个进程（同 ext:result / ext:input 兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::platform::ai_chat::chat_window(hwnd as isize, &url, &model, &key, &sys);
                }));
                if r.is_err() {
                    return Err(mlua::Error::external(
                        "AI 对话窗口异常退出（已避免程序崩溃）",
                    ));
                }
                Ok(())
            },
        );
    }
}

/// `serde_json::Value` → Lua 值（`ext:json_decode` 的主体）。
fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => mlua::Value::Nil,
        J::Bool(b) => mlua::Value::Boolean(*b),
        J::Number(n) => match n.as_f64() {
            Some(f) => mlua::Value::Number(f),
            None => mlua::Value::Nil,
        },
        J::String(s) => mlua::Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, item) in a.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, val) in o {
                t.set(k.as_str(), json_to_lua(lua, val)?)?;
            }
            mlua::Value::Table(t)
        }
    })
}

/// Lua 值 → `serde_json::Value`（`ext:json_encode` 的主体）。
///
/// 表的分流判据：`1..=len` 每个位置都有值 → 数组；否则按字符串键 → 对象。
/// 递归深度极浅的脚本数据够用；函数 / userdata 之类不支持的值一律落 `null`。
fn lua_to_json(v: &mlua::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        mlua::Value::Nil => J::Null,
        mlua::Value::Boolean(b) => J::Bool(*b),
        mlua::Value::Integer(i) => J::Number((*i).into()),
        mlua::Value::Number(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        mlua::Value::String(s) => J::String(s.to_string_lossy()),
        mlua::Value::Table(t) => {
            let len = t.raw_len();
            let mut arr = Vec::with_capacity(len);
            let mut is_array = len > 0;
            for i in 1..=len {
                match t.get::<mlua::Value>(i) {
                    Ok(val) if !matches!(val, mlua::Value::Nil) => arr.push(lua_to_json(&val)),
                    _ => {
                        is_array = false;
                        break;
                    }
                }
            }
            if is_array {
                return J::Array(arr);
            }
            let mut obj = serde_json::Map::new();
            for pair in t.pairs::<String, mlua::Value>().flatten() {
                let (k, val) = pair;
                obj.insert(k, lua_to_json(&val));
            }
            J::Object(obj)
        }
        _ => J::Null,
    }
}

/// 阻塞 HTTP。错误 → `(0, 错误信息)`（脚本据此判断失败，不 panic）。
fn http_blocking(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> (i64, String) {
    // ★ 必须用平台层的统一构造点（TLS provider 只在那儿配），不得在此自配 Agent。
    let agent = crate::platform::http::agent(Duration::from_secs(60));
    let m = method.to_uppercase();
    // GET/DELETE（无 body）与 POST/PUT（带 body）在 ureq 3 里是两种不同构造器
    // （WithoutBody / WithBody），不能在同一处 match 统一成一句话——这里每个方法各走
    // 各的构造与发送，返回值再统一解析。
    let result = match m.as_str() {
        "GET" => {
            let mut req = agent.get(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        "DELETE" => {
            let mut req = agent.delete(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        "POST" => {
            let mut req = agent.post(url).header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            // 在线接口公共约定按 JSON 发；body 没有就发空串（保持类型一致）。
            req.send(body.map(|b| b.to_string()).unwrap_or_default())
        }
        "PUT" => {
            let mut req = agent.put(url).header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.send(body.map(|b| b.to_string()).unwrap_or_default())
        }
        _ => {
            return (0, format!("不支持的 HTTP 方法：{m}"));
        }
    };
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16() as i64;
            match resp.into_body().read_to_string() {
                Ok(body) => (status, body),
                Err(e) => (status, format!("读取响应失败：{e}")),
            }
        }
        Err(e) => (0, format!("请求失败：{e}")),
    }
}

/// `ext:get_image()` 返回的贴图 userdata：只读尺寸 + `png_base64()` / `save(path)` /
/// `pixel(x, y)` / `bytes()`（脚本**只读**——拿到的是贴图快照的克隆，改动不会回写贴图）。
pub struct ExtImage {
    img: RgbaBitmap,
}

/// 把一张位图包成 `ExtImage` 的 Lua userdata（统一框架里给 `ctx.image` 用，与
/// `ext:get_image()` 同一个对象类型）。
pub fn ext_image_value(lua: &Lua, img: RgbaBitmap) -> mlua::Result<mlua::Value> {
    lua.create_userdata(ExtImage { img })
        .map(mlua::Value::UserData)
}

impl UserData for ExtImage {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        // `.w` / `.h` 只读字段（脚本里 `img.w` 取宽）。字段访问用 `add_fields` 的 getter。
        fields.add_field_method_get("w", |_lua, this| Ok(this.img.w));
        fields.add_field_method_get("h", |_lua, this| Ok(this.img.h));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("png_base64", |_lua, this, ()| {
            base64_png(&this.img).map_err(mlua::Error::external)
        });
        methods.add_method("save", |_lua, this, path: String| {
            save_png(&this.img, &path).map_err(mlua::Error::external)
        });
        // pixel(x, y) -> r, g, b, a（越界返回全 nil，脚本自己判）。逐点访问适合小图
        // 或稀疏采样；整图遍历请用下面的 bytes()，一次取回原始缓冲。
        methods.add_method("pixel", |_lua, this, (x, y): (i64, i64)| {
            if x < 0 || y < 0 || x >= this.img.w as i64 || y >= this.img.h as i64 {
                return Ok(mlua::MultiValue::new());
            }
            let idx = (y as u32 * this.img.w + x as u32) as usize * 4;
            let px = &this.img.rgba[idx..idx + 4];
            Ok(mlua::MultiValue::from_vec(
                px.iter()
                    .map(|b| mlua::Value::Integer(i64::from(*b)))
                    .collect(),
            ))
        });
        // bytes() -> 原始 RGBA 缓冲（Lua 二进制安全字符串，长度 = w*h*4，行优先）。
        // 采样统计类脚本用它 + string.byte 批量读，比逐点 pixel() 快一个量级。
        methods.add_method("bytes", |lua, this, ()| {
            Ok(mlua::Value::String(lua.create_string(&this.img.rgba)?))
        });
    }
}

/// base64 编码表。
const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码（含 `=` 填充）。无现成 crate，手写一份，够 PNG 用。
fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(B64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(B64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// RGBA 位图 → base64 编码的 PNG 文本（在线接口用）。复用截图 PNG 编码链。
fn base64_png(img: &RgbaBitmap) -> Result<String, String> {
    let shot =
        crate::core::backdrop::bitmap_to_shot(img).ok_or_else(|| "转换位图格式失败".to_string())?;
    let png =
        crate::platform::screenshot::encode_png(&shot).map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(b64(&png))
}

/// RGBA 位图 → 存成 PNG 文件（父目录不存在则创建）。
fn save_png(img: &RgbaBitmap, path: &str) -> Result<(), String> {
    let shot =
        crate::core::backdrop::bitmap_to_shot(img).ok_or_else(|| "转换位图格式失败".to_string())?;
    crate::platform::screenshot::save_png(&shot, std::path::Path::new(path))
        .map_err(|e| format!("保存失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn no_context_get_image_returns_nil() {
        // 清掉线上残留上下文（测试隔离）：这里线程局部是空的。
        clear_ext_context();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let script = "local i = ext:get_image() return i == nil";
        let ok: bool = lua.load(script).eval().unwrap();
        assert!(ok, "无贴图上下文时应返回 nil");
    }

    #[test]
    fn pixel_returns_rgba_and_nil_out_of_range() {
        clear_ext_context();
        // 2×1 图：像素(0,0)=红 (255,0,0,255)，(1,0)=半透明绿 (0,255,0,128)。
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 2,
                h: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
            },
            hwnd: 0,
        });
        let img: mlua::Value = lua.load("return ext:get_image()").eval().unwrap();
        lua.globals().set("img", img).unwrap();
        let (r, g, b, a): (i64, i64, i64, i64) = lua
            .load("return img:pixel(1, 0)")
            .set_name("t")
            .eval()
            .unwrap();
        assert_eq!((r, g, b, a), (0, 255, 0, 128));
        let (r, g, b, a): (i64, i64, i64, i64) = lua
            .load("return img:pixel(0, 0)")
            .set_name("t")
            .eval()
            .unwrap();
        assert_eq!((r, g, b, a), (255, 0, 0, 255));
        // 越界（负数 / 超宽 / 超高）→ 返回空（全 nil），脚本据此自判。
        for code in [
            "return img:pixel(-1, 0)",
            "return img:pixel(2, 0)",
            "return img:pixel(0, 1)",
        ] {
            let vals: mlua::MultiValue = lua.load(code).eval().unwrap();
            assert!(vals.is_empty(), "{code} 越界应返回空（全 nil）");
        }
    }

    #[test]
    fn bytes_length_is_w_times_h_times_4() {
        clear_ext_context();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 3,
                h: 2,
                rgba: (0..24).collect(),
            },
            hwnd: 0,
        });
        let (len, first, last): (i64, i64, i64) = lua
            .load("local img = ext:get_image() local d = img:bytes() return #d, d:byte(1), d:byte(-1)")
            .eval()
            .unwrap();
        assert_eq!(len, 24, "长度必须 = w*h*4");
        assert_eq!((first, last), (0, 23), "内容按 RGBA 行优先原样给出");
    }

    #[test]
    fn png_base64_has_png_signature() {
        clear_ext_context();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 2,
                h: 2,
                rgba: vec![255u8; 16],
            },
            hwnd: 0,
        });
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let b64: String = lua
            .load("local img = ext:get_image() return img:png_base64()")
            .eval()
            .unwrap();
        clear_ext_context();
        assert!(!b64.is_empty());
        // base64 解回前 4 字节必须是 PNG 签名 0x89 'P' 'N' 'G'。
        let bytes = b64decode(&b64);
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
    }

    /// 最小 base64 解码（仅测试用，够验 PNG 签名即可）。
    fn b64decode(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u8;
        for c in s.chars() {
            let v = match c {
                'A'..='Z' => c as u32 - 'A' as u32,
                'a'..='z' => c as u32 - 'a' as u32 + 26,
                '0'..='9' => c as u32 - '0' as u32 + 52,
                '+' => 62,
                '/' => 63,
                _ => continue,
            };
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
}
