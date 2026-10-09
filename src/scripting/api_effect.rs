//! 暴露给 Lua 的效果上下文 API。包括随机数、屏幕信息、颜色助手。

use mlua::Lua;

/// 注册效果上下文相关的全局变量和函数。
pub fn register_effect_api(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();

    // hex_to_rgb("#RRGGBB" / "#RRGGBBAA") -> (r, g, b, a)。
    //
    // ★★ 8 位的字节顺序是 **R,G,B,A**（与 `Argb::parse_hex`、windui
    //   `Color::to_hex_string`、`config.json` 同一口径）。2026-09-25 前这里按
    //   `#AARRGGBB` 拆，于是脚本里 `default = "#FF000040"`（红 + 25% 透明）被读成
    //   不透明深蓝、`"#00FFCCBF"` 被读成全透明——画出来颜色完全不对。
    //
    // ★ 第四个返回值（2026-09-25 加）：颜色参数可以声明 `alpha = true` 让取色器带上
    //   透明度条，此时落盘是 8 位 `#RRGGBBAA`，脚本靠这个 a 拿到"颜色自带的透明度"。
    //   6 位串一律返回 255——这类脚本的透明度由独立的「不透明度」参数管，两套并存
    //   会互相打架（见 `ParamDef::alpha`）。
    //   老脚本写 `local r, g, b = hex_to_rgb(c)` 完全不受影响（多返回值自动截断）。
    globals.set(
        "hex_to_rgb",
        lua.create_function(|_, hex: String| -> mlua::Result<(u8, u8, u8, u8)> {
            Ok(parse_hex_rgba(&hex))
        })?,
    )?;

    // 屏幕尺寸（由 Rust 端每帧更新）
    globals.set("screen_w", 1920f32)?;
    globals.set("screen_h", 1080f32)?;

    // 时间（由 Rust 端每帧更新）
    globals.set("elapsed", 0f32)?;

    // 指针位置（由 Rust 端每帧更新）。
    //
    // ★ `cursor_x/y` 是**正名**，`mouse_x/y` 是它的**别名**（老脚本零改动）。
    //   此前点击效果宿主把 `mouse_x/y` 恒传 0，误用它的脚本会静默画到屏幕左上角；
    //   统一后四类宿主都填真实指针（点击效果里 = 按下点）。见 `engine::update_env`。
    globals.set("cursor_x", 0f32)?;
    globals.set("cursor_y", 0f32)?;
    globals.set("mouse_x", 0f32)?;
    globals.set("mouse_y", 0f32)?;

    // 实例出生点：拖尾 = 首个采样点 / 点击效果 = 按下点 / 悬浮标 = 当前指针位置。
    // 脚本要在"出生处"画东西时用它，不必自己靠 on_press 记。
    globals.set("origin_x", 0f32)?;
    globals.set("origin_y", 0f32)?;

    // 当前宿主标识：`trail` / `hover` / `clickfx` / `panel`（由 Rust 端每帧更新）。
    globals.set("host", "")?;

    // rng(min, max) -> float  随机浮点数
    //
    // ★ 空区间必须自己兜住：`rand::gen_range` 遇 `min >= max` 会 **panic**
    // （"cannot sample empty range"），而 panic 发生在渲染帧里等于整个进程闪退——
    // 蛛丝缠网的 `rng(1.0, math.max(1.0, 丝线粗细))` 在粗细取到 schema 下限 1 时正好
    // 命中这条（2026-09-15 用户报告的"切换蛛丝缠网崩溃"）。脚本作者不该为一个随机数
    // 操心 rand 的 panic 语义，故在 API 这一层退化：区间为空时返回 `min`。
    //
    // NaN 同样要拦：`min..max` 含 NaN 时 rand 也会 panic，而脚本里的除法/上限计算
    // 完全可能产出 NaN（例如寿命为 0 时的比例）。
    globals.set(
        "rng",
        lua.create_function(|_, (min, max): (f32, f32)| -> mlua::Result<f32> {
            use rand::Rng;
            // 写成 `partial_cmp(..) != Greater` 而不是 `!(max > min)`：后者会撞 clippy 的
            // `neg_cmp_op_on_partial_ord`（浮点是部分序），而这里**恰恰需要**把 NaN 也算进
            // "区间不可用"——`min..max` 含 NaN 时 rand 同样 panic。
            if max.partial_cmp(&min) != Some(std::cmp::Ordering::Greater) {
                return Ok(if min.is_nan() { 0.0 } else { min });
            }
            let mut rng = rand::thread_rng();
            Ok(rng.gen_range(min..max))
        })?,
    )?;

    // rng_int(min, max) -> int  随机整数（闭区间）
    //
    // 同上：`min..=max` 在 `min > max` 时也会 panic，退化成返回 `min`。
    globals.set(
        "rng_int",
        lua.create_function(|_, (min, max): (i32, i32)| -> mlua::Result<i32> {
            use rand::Rng;
            if max <= min {
                return Ok(min);
            }
            let mut rng = rand::thread_rng();
            Ok(rng.gen_range(min..=max))
        })?,
    )?;

    // math 辅助：clamp
    globals.set(
        "clamp",
        lua.create_function(|_, (v, lo, hi): (f32, f32, f32)| -> mlua::Result<f32> {
            Ok(v.clamp(lo, hi))
        })?,
    )?;

    // math 辅助：lerp
    globals.set(
        "lerp",
        lua.create_function(|_, (a, b, t): (f32, f32, f32)| -> mlua::Result<f32> {
            Ok(a + (b - a) * t.clamp(0.0, 1.0))
        })?,
    )?;

    Ok(())
}

/// 解析 `#RRGGBB` / `#RRGGBBAA` → (r, g, b, a)。
///
/// ★ **只按字节解析**（2026-09-29）：此前的 `&h[0..2]` 系 str 切片在输入含多字节
///   UTF-8 字符时会撞「byte index not on char boundary」panic——脚本作者完全可能
///   写 `hex_to_rgb("红色")`（len=6 恰好命中 6 位分支），而 panic 发生在渲染帧里
///   等于整个进程闪退。字节级解析对**任意**输入都不 panic，非法输入逐字节兜底。
fn parse_hex_rgba(hex: &str) -> (u8, u8, u8, u8) {
    fn nib(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    /// 两位合一个通道；任一字符非法取 `fallback`（RGB 通道 0、A 通道 255，
    /// 与旧的 `from_str_radix(..).unwrap_or(..)` 语义逐字等价）。
    fn pair(hi: u8, lo: u8, fallback: u8) -> u8 {
        match (nib(hi), nib(lo)) {
            (Some(h), Some(l)) => h << 4 | l,
            _ => fallback,
        }
    }
    let bytes = hex.trim_start_matches('#').as_bytes();
    match bytes.len() {
        8 => (
            pair(bytes[0], bytes[1], 0),
            pair(bytes[2], bytes[3], 0),
            pair(bytes[4], bytes[5], 0),
            pair(bytes[6], bytes[7], 255),
        ),
        6 => (
            pair(bytes[0], bytes[1], 0),
            pair(bytes[2], bytes[3], 0),
            pair(bytes[4], bytes[5], 0),
            255,
        ),
        _ => (255, 0, 0, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_hex_rgba;

    #[test]
    fn parses_six_and_eight_digit_forms() {
        assert_eq!(parse_hex_rgba("#FF8000"), (255, 128, 0, 255));
        assert_eq!(parse_hex_rgba("#FF800080"), (255, 128, 0, 128));
        assert_eq!(parse_hex_rgba("FF8000"), (255, 128, 0, 255));
        assert_eq!(parse_hex_rgba("ff8000"), (255, 128, 0, 255));
    }

    #[test]
    fn malformed_input_falls_back_without_panic() {
        // 长度不对 → 整体兜底。
        assert_eq!(parse_hex_rgba(""), (255, 0, 0, 255));
        assert_eq!(parse_hex_rgba("#FFF"), (255, 0, 0, 255));
        // 6/8 字节但含非 hex 字符 → 逐通道兜底（RGB 0、A 255）。
        assert_eq!(parse_hex_rgba("zz8000"), (0, 128, 0, 255));
        assert_eq!(parse_hex_rgba("#FF8000zz"), (255, 128, 0, 255));
    }

    #[test]
    fn multibyte_utf8_input_never_panics() {
        // 回归锁：旧实现 `&h[0..2]` 撞「红」的字节中间会 panic。
        // "红色" = 6 字节恰好命中 6 位分支；"红a1b2c3" = 9 字节走长度兜底。
        let _ = parse_hex_rgba("红色a1b2"); // 10 字节 → 长度兜底
                                            // "红"=E7 BA A2、"色"=E8 89 B2：6 字节全部非 hex 字符 → 逐通道兜底 (0,0,0,255)。
        assert_eq!(parse_hex_rgba("红色"), (0, 0, 0, 255));
        assert_eq!(parse_hex_rgba("红a1b2c3"), (255, 0, 0, 255)); // 9 字节 → 长度兜底
    }
}
