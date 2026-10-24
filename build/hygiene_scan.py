r"""静态异味体检（只读）——对齐《开发指南.md》第四节「静态异味扫描」。

用法：
  python build/hygiene_scan.py            # 违规清单 + 汇总
  python build/hygiene_scan.py --json     # 机器可读

七查（第 1~6 查只扫第一方 Rust 源码 `main/src/**/*.rs`；第 7 查扫**全树**）：

  1. 硬编码绝对路径  `[A-Z]:\`  —— 换台电脑就失效
  2. 调试残留        `println!` / `eprintln!` / `print!` / `eprint!` / `dbg!`
  3. 未完成标记      `TODO` / `FIXME` / `XXX`（只认 `TODO:` / `TODO(` 这类**真标记**）
  4. 裸 `OpenClipboard` —— 剪贴板是全系统互斥资源，且 Windows **不对同进程内的
     并发打开提供互斥**（探针 `tests/probes/clip_race.rs` 实测：两个线程能同时打开）。
     2026-10-12 的 0xC0000005 就是自家写者的 `EmptyClipboard()` 把读者正在 memcpy
     的那块内存抽走。任何访问都必须经 `platform::clipboard::with_clipboard` /
     `lock_clip`，所以**只有 `src/platform/clipboard.rs` 里允许出现**。
  5. struct literal 里的临时宽字符缓冲 —— `lpszClassName: PCWSTR(wide(x).as_ptr())`
     这种写法里，`wide()` 的临时 `Vec<u16>` 在这条 `let` 语句的**分号处**就被 drop，
     下一句 `RegisterClassExW` 拿到野指针；且因为是 `.as_ptr()`，借用检查器看不见。
     2026-10-12 的「找不到窗口类 → 僵尸进程」就是它。必须先 `let name_w = wide(x);`。
  6. **指针类 unsafe 缺 `// SAFETY:`** —— `transmute` / `from_raw` / `get_unchecked` /
     `copy_nonoverlapping` / `read|write_unaligned`。这类操作的前提编译器一个都查不了
     （越界、对齐、生命周期、别名全靠人），所以注释不是"风格"而是**义务与证据**：
     写下它才迫使你想清楚前提是什么。判据：该行或其上方 4 行内出现 `SAFETY`
     （4 是实测的最小够用值：常见排版是「SAFETY 注释 → `unsafe {` → 调用名 →
     指针参数」，正好差 4 行）。

  7. **本机真实路径泄漏（全树）** —— 硬编码绝对路径换台电脑就失效是小恶；
     把开发机的目录结构（工作目录名 / 真实用户名）发进公开仓库是大恶。
     判据只认**只可能来自本机**的指纹（工作目录名、`C:\Users\<真实用户名>\`），
     占位示例（`C:\App`、`D:\素材`、`C:\Users\someone`）一律不命中。
     覆盖 `main/` 全部文本类文件（源码 / 文档 / 脚本 / 配置），豁免本文件自身
     （模式定义处不含明文指纹，豁免是双保险）。

豁免（与 Pecia 开发指南第四节口径一致）：
  - `#[cfg(test)]` 之后的代码是测试桩：允许硬编路径与往 stdout 打印。
  - **纯注释行**里出现的调试宏字样（文档性提及）不算调用；同理 TODO 只在注释里才有意义。
  - `src/core/logger.rs`：日志系统自身的兜底输出必须能落到 stderr，那唯一一处 `eprintln!` 合法。
  - 第 4 查：`src/platform/clipboard.rs`（门禁实现所在）豁免；注释行豁免。

退出码：0 = 「违规 0 处」；1 = 有违规。

本脚本只做静态字符串扫描，不引第三方正则库；不做语义解析（`#[cfg(test)]` 之后的
代码统一按测试桩处理——本项目惯例是「生产代码在前、`mod tests` 在文件末尾」）。
"""
import json
import pathlib
import re
import sys

ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent  # <main>/
SRC = ROOT_DIR / "src"

# `#[cfg(test)]` 之后 = 测试区。
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
# 硬编码绝对路径（盘符 + 反斜杠）。只匹配**一个**反斜杠，于是同时覆盖
# `"C:\\x"`（Rust 转义写法）与 `r"C:\x"`（raw string）两种源码形态。
ABS_PATH = re.compile(r"[A-Za-z]:\\")
# 调试宏调用。
DEBUG_MACRO = re.compile(r"\b(?:println|eprintln|print|eprint|dbg)!\s*[\(\[]")
# 真·未完成标记：半角冒号或左括号紧跟（排除「这里以前是 TODO：…」这类全角叙述）。
MARKER = re.compile(r"\b(?:TODO|FIXME|XXX)\s*[:(]")

# 允许直接往 stderr 写的唯一模块（日志系统的自举兜底）。
LOGGER_EXEMPT = {"src/core/logger.rs"}

# ── 第 4 查：裸 `OpenClipboard`（剪贴板门禁） ─────────────────────────────
# 只有门禁实现所在的文件能直接开剪贴板；别处一律走 with_clipboard / lock_clip。
CLIP_GATE_EXEMPT = {"src/platform/clipboard.rs"}
# 只认真实调用（后面跟 `(`），避免把注释里的字样算进来（注释另行豁免）。
BARE_OPEN_CLIPBOARD = re.compile(r"\bOpenClipboard\s*\(")

# ── 第 5 查：struct literal 里的临时宽字符缓冲 ─────────────────────────────
# `lpszClassName: PCWSTR(wide(..).as_ptr())` / `PCWSTR(cls.as_ptr())` 一律拦：
# 后者若 `cls` 是上一行 `let` 出来的局部变量其实安全，但本项目的规矩是"类名走
# `w!` 静态串或显式 `let` 绑定后再取址"，这里宁可严一点——真有需要就用 `w!`。
TEMP_WIDE_IN_STRUCT = re.compile(
    r"^\s*lpsz\w*[Nn]ame\s*:\s*(?:windows::core\.)?PCWSTR\(\s*wide\("
)

# ── 第 6 查：指针类 unsafe 缺 `// SAFETY:` ────────────────────────────────
# 只卡**指针类**：那才是 UB 高发区（越界 / 对齐 / 生命周期 / 别名，编译器全都看不见）。
# 普通 FFI 调用的 unsafe（`GetCursorPos(&mut p)` 之类）前提是"参数类型对不对"，
# 由签名保证，逐个写 SAFETY 的收益远低于噪音——存量 300+ 处，留作逐步清零
# （见 docs/DESIGN_ADOPTION.md D3）。
PTR_UNSAFE = re.compile(
    r"\b(?:transmute|from_raw|get_unchecked|copy_nonoverlapping"
    r"|read_unaligned|write_unaligned)\b"
)
# 往上找几行内的 SAFETY 说明（注释常常写在 unsafe 块的上方）。
SAFETY_LOOKBACK = 4

# ── 第 7 查：本机真实路径泄漏（全树） ──────────────────────────────────────
# 指纹只认"只可能来自本机"的两种形态：开发机工作目录名、真实用户目录
# （带尾分隔符，避免误伤 user2 / username 这类合法用户名）。
LEAK_FINGERPRINTS = [
    re.compile(r"AI\.Agent"),
    re.compile(r"Users[\\/]+user[\\/]"),
]
SCAN_EXTS = {
    ".rs", ".md", ".py", ".bat", ".cmd", ".toml", ".json", ".lua", ".txt",
    ".ps1", ".yml", ".yaml", ".rc", ".cfg", ".xml", ".html", ".css", ".js", ".sh",
}
# 本文件的模式定义不含明文指纹（正则形态不匹配自身）；豁免是防将来改动时自伤。
SCAN_SELF_EXEMPT = {"build/hygiene_scan.py"}


def is_comment(line: str) -> bool:
    return line.lstrip().startswith("//")


def has_safety(lines: list, idx: int) -> bool:
    """`lines[idx]`（0-based）及其上方 SAFETY_LOOKBACK 行里有没有 `SAFETY` 说明。"""
    lo = max(0, idx - SAFETY_LOOKBACK)
    for j in range(lo, idx + 1):
        if "SAFETY" in lines[j]:
            return True
    return False


def main():
    violations = []
    for p in sorted(SRC.rglob("*.rs")):
        rel = p.relative_to(ROOT_DIR).as_posix()
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        in_test = False
        for i, line in enumerate(lines, 1):
            if CFG_TEST.match(line):
                in_test = True
            if in_test:
                continue  # 测试桩豁免
            comment = is_comment(line)
            if ABS_PATH.search(line) and not comment:
                violations.append(f"{rel}:{i}: 硬编码绝对路径 —— {line.strip()[:80]}")
            if DEBUG_MACRO.search(line) and not comment and rel not in LOGGER_EXEMPT:
                violations.append(f"{rel}:{i}: 调试残留 —— {line.strip()[:80]}")
            if comment and MARKER.search(line):
                violations.append(f"{rel}:{i}: 未完成标记 —— {line.strip()[:80]}")
            if (
                BARE_OPEN_CLIPBOARD.search(line)
                and not comment
                and rel not in CLIP_GATE_EXEMPT
            ):
                violations.append(
                    f"{rel}:{i}: 裸 OpenClipboard —— 剪贴板必须经 "
                    f"platform::clipboard::with_clipboard / lock_clip —— {line.strip()[:80]}"
                )
            if TEMP_WIDE_IN_STRUCT.search(line) and not comment:
                violations.append(
                    f"{rel}:{i}: struct literal 里的临时宽字符缓冲 —— 先 `let n = wide(x);` "
                    f"再取址，否则 RegisterClass 拿到野指针 —— {line.strip()[:80]}"
                )
            # 第 6 查：指针类 unsafe 必须有 `// SAFETY:`（本行或上方 4 行内）。
            # 注释行自己带着关键字样是常见写法（`// SAFETY: ...transmute...`），要豁免，
            # 否则"写了说明"反而被判违规。
            if PTR_UNSAFE.search(line) and not comment and not has_safety(lines, i - 1):
                violations.append(
                    f"{rel}:{i}: 指针类 unsafe 缺 `// SAFETY:` —— 写清你凭什么认为它安全 "
                    f"（越界 / 对齐 / 生命周期 / 别名）—— {line.strip()[:80]}"
                )

    # ── 第 7 查：本机真实路径泄漏（全树，不只 src） ────────────────────────
    for p in sorted(ROOT_DIR.rglob("*")):
        if not p.is_file() or p.suffix.lower() not in SCAN_EXTS:
            continue
        rel = p.relative_to(ROOT_DIR).as_posix()
        if rel in SCAN_SELF_EXEMPT:
            continue
        try:
            text = p.read_text(encoding="utf-8", errors="strict")
        except (UnicodeDecodeError, OSError):
            continue  # 二进制 / 非 UTF-8 文件不在本查范围
        for i, line in enumerate(text.splitlines(), 1):
            for pat in LEAK_FINGERPRINTS:
                if pat.search(line):
                    violations.append(
                        f"{rel}:{i}: 本机真实路径泄漏 —— 开发机的目录名 / 真实用户名"
                        f"不得进仓库，改用相对或 CARGO_MANIFEST_DIR 派生路径 —— "
                        f"{line.strip()[:80]}"
                    )

    if "--json" in sys.argv:
        print(json.dumps({"violations": violations, "count": len(violations)},
                         ensure_ascii=False, indent=2))
    else:
        if violations:
            for v in violations:
                print(" [" + "✗" + "] " + v)
            print(f"\n违规 {len(violations)} 处")
            return 1
        print("违规 0 处")
    return 0


if __name__ == "__main__":
    sys.exit(main())
