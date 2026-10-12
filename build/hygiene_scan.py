r"""静态异味体检（只读）——对齐《开发指南.md》第四节「静态异味扫描」。

用法：
  python build/hygiene_scan.py            # 违规清单 + 汇总
  python build/hygiene_scan.py --json     # 机器可读

五查（只扫第一方 Rust 源码 `main/src/**/*.rs`）：

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


def is_comment(line: str) -> bool:
    return line.lstrip().startswith("//")


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
