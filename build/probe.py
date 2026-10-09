#!/usr/bin/env python3
"""真机探针的按需登记与运行（配合 `Cargo.toml` 里"默认不登记"的做法）。

# 为什么要有这个脚本

`tests/probes/` 下的十一条探针**不登记在 `Cargo.toml` 里**——这是刻意的
（理由见 `Cargo.toml` 同名注释块，2026-10-05 实测）：

| 配置 | `cargo test --no-run` 编出的二进制 | 耗时 |
|---|---|---|
| 登记探针（`[[test]]`） | 16 个 | 1 分 55 秒 |
| 不登记（默认） | 5 个 | 1 分 29 秒 |

差的 **26 秒 / 每次改动**里**一个断言都没跑**：十一条全是 `#[ignore]`，
要真实声卡 / DXGI 采集 / 真实窗口。日常编译是为了快速验证改动，不该为它们等。

代价是 `cargo test --test capture_smoke` 这条命令**默认不可用**。本脚本补上它：
临时把登记写进 `Cargo.toml`、跑完**无条件还原**（含 Ctrl-C / 异常 / 编译失败）。

# 用法

```text
python build/probe.py list                      # 列出全部探针（带快慢分组）
python build/probe.py run capture_smoke         # 跑一个（推荐先 --list 看名字）
python build/probe.py run capture_smoke -- --nocapture
python build/probe.py run fast                  # 只跑快组（秒级，回归首选）
python build/probe.py run all                   # 全跑（很慢：逐个真录/真开窗）
python build/probe.py run capture_smoke --secs 300   # 给长录类定时长
```

# 快慢分组（2026-10-08）

★ **测试时长是硬约束**：录制类探针默认偏长（`rec_load60` 默认 300s、`crash_loss`
硬等 90s、`rec_tone` 48s），回归时不要无脑 `run all`。分组标记写在探针源码头部
（`//! probe-group: fast`），没写标记的一律算 **slow**：

- **fast**：不录制、不硬等，秒级到十几秒（几何 / 像素 / 离线 PCM / 抓屏可见性）。
  改了窗口、几何、菜单、便签、倍率这类东西，跑 `run fast` 就够。
- **slow**：真录一段 / 真等一个超时 / 要真 TIM 窗口。只对相关改动跑，
  且**先用 `--secs` 把时长压到最短**（能 20 秒看清的别跑 300 秒）。

★ 录制类探针**必须**串行（`--test-threads=1`）：DXGI 桌面复制同一时刻全进程只容
一份，并发会互相抢，症状看起来像"录屏坏了"其实只是并发。脚本默认已带该参数。

# 为什么不用 `[[example]]` + `required-features`

试过，行不通（详见 `Cargo.toml` 注释）：`[[example]]` 必须有 `fn main`，而探针是
`#[test]` 集合；改完 `cargo test --test capture_smoke` 就不存在了 ——
拿"跑不了"换"省编译时间"，不划算。
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

# 以本文件位置定位仓库根（main/build/probe.py → main/）
HERE = os.path.dirname(os.path.abspath(__file__))
MAIN = os.path.dirname(HERE)
CARGO_TOML = os.path.join(MAIN, "Cargo.toml")
PROBE_DIR = os.path.join(MAIN, "tests", "probes")

# 插到 [dependencies] 之前的锚点（与 Cargo.toml 里的注释块配套）
ANCHOR = "[dependencies]"


def probe_names() -> list[str]:
    """探针名 = tests/probes/*.rs 的文件名（去扩展名），按字典序。

    刻意用文件系统扫描而不是在脚本里写死清单：新增探针时忘了改这里，
    就会出现"探针存在但 --test 跑不到"—— 而那正是本脚本要消灭的毛病。
    """
    if not os.path.isdir(PROBE_DIR):
        return []
    return sorted(
        os.path.splitext(f)[0] for f in os.listdir(PROBE_DIR) if f.endswith(".rs")
    )


FAST = "fast"
SLOW = "slow"


def probe_group(name: str) -> str:
    """读探针头部约定的分组标记：`//! probe-group: fast`。

    没写标记的一律算 slow——**保守方向很重要**：一个"其实要录 90 秒"的探针被误判
    成 fast 混进回归，用户等的就是它；反过来只是多问一句。标记由探针自己声明，
    新增探针不改本脚本也能用（与 `probe_names` 同样的理由：不写死清单）。
    """
    path = os.path.join(PROBE_DIR, f"{name}.rs")
    try:
        with open(path, encoding="utf-8", newline="") as f:
            head = "".join(f.readlines()[:40])
    except OSError:
        return SLOW
    m = re.search(r"^//!\s*probe-group:\s*(\w+)", head, re.M)
    if not m or m.group(1).lower() != FAST:
        return SLOW
    return FAST


def read_text(p: str) -> str:
    with open(p, encoding="utf-8", newline="") as f:
        return f.read()


def write_text(p: str, s: str) -> None:
    # ★newline=""：这些 .py/.toml 在本仓是 LF。用默认换行会随平台改写，
    #   在 Windows 上可能把 LF 写成 CRLF，制造"只有我这儿有 diff"的假象。
    with open(p, "w", encoding="utf-8", newline="") as f:
        f.write(s)


def build_registration(names: list[str]) -> str:
    """生成临时 `[[test]]` 登记段。"""
    lines = [
        "# ── 临时探针登记（由 build/probe.py 写入，跑完自动还原）──",
        "# 平时不登记是有意的：它们全是 #[ignore]、要真实硬件，每次改动白编 26 秒。",
    ]
    for n in names:
        lines += ["[[test]]", f'name = "{n}"', f'path = "tests/probes/{n}.rs"']
    return "\n".join(lines) + "\n\n"


def register(names: list[str]) -> str:
    """把登记写进 Cargo.toml。返回原始内容，供还原用。"""
    orig = read_text(CARGO_TOML)
    if ANCHOR not in orig:
        sys.exit(f"FAIL 在 {CARGO_TOML} 里找不到锚点 {ANCHOR}")
    # ★ 检查"是否已登记"必须按**行首整行**匹配，不能用 `"[[test]]" in orig`——
    #   `[[test]]` 这个字符串在上面的注释块里出现过N 次，那样判断永远为真、
    #   每次都误报"已有登记"。这属于"判据比被测物松/严"的同类坑。
    if re.search(r"^\[\[test\]\]$", orig, re.M):
        print("注意：Cargo.toml 里已有 [[test]] 登记；本脚本仍会写入自己的段，"
              "还原时按整体覆盖处理")
    write_text(CARGO_TOML, orig.replace(ANCHOR, build_registration(names) + ANCHOR, 1))
    return orig


def restore(orig: str) -> None:
    """无条件还原。"""
    write_text(CARGO_TOML, orig)
    print("已还原 Cargo.toml（探针退出默认编译路径）")


def run_probe(name: str, extra: list[str], secs: int | None) -> int:
    names = probe_names()
    if name not in names:
        print(f"没有叫 {name} 的探针。可用：{', '.join(names)}")
        return 2

    # 长录类用例靠环境变量定时长（见 capture_smoke.rs 的 CURSEEN_LONG_REC_S）
    env = dict(os.environ)
    if secs is not None:
        env["CURSEEN_LONG_REC_S"] = str(secs)
        print(f"长录时长设为 {secs}s（CURSEEN_LONG_REC_S）")

    orig = register([name])
    try:
        cmd = [
            "cargo", "test", "--release",
            "--test", name,
            "--", "--ignored", "--nocapture", "--test-threads=1",
        ] + extra
        print("$ " + " ".join(cmd))
        # ★ 不接stdin：探针要真鼠标输入 / 真窗口，接了会把父进程的 stdin 抢走。
        return subprocess.call(cmd, cwd=MAIN, env=env)
    finally:
        # ★ 无论如何都还原：编译失败、Ctrl-C、探针 panic 都算。
        #   漏还原会把探针留在默认编译路径上 —— 那正是本脚本要避免的状态。
        restore(orig)


def _pop_secs(argv: list[str]) -> tuple[list[str], int | None]:
    """把 `--secs N` / `--secs=N` 从命令行里摘出来。

    必须**前置摘除**：`passthrough` 是 `argparse.REMAINDER`，出现在 target 之后的
    选项它整段吞下、argparse 永远见不到——`--secs 300` 透传给 cargo 会报
    `Unrecognized option: 'secs'`（2026-10-07）。在这里摘，main 里直接用返回值。
    """
    out: list[str] = []
    secs: int | None = None
    i = 0
    while i < len(argv):
        tok = argv[i]
        if tok == "--secs" and i + 1 < len(argv):
            secs = int(argv[i + 1])
            i += 2
            continue
        if tok.startswith("--secs="):
            secs = int(tok.split("=", 1)[1])
            i += 1
            continue
        out.append(tok)
        i += 1
    return out, secs


def main() -> int:
    ap = argparse.ArgumentParser(
        description="真机探针按需登记与运行（平时不登记，见 Cargo.toml 注释）"
    )
    ap.add_argument("action", choices=["list", "run"])
    ap.add_argument("target", nargs="?", help="探针名，或 all")
    ap.add_argument("passthrough", nargs=argparse.REMAINDER, help="-- 之后原样转给 cargo test")
    ap.add_argument("--secs", type=int, help="长录时长（给 CURSEEN_LONG_REC_S）")
    # ★ `--secs` 必须自己先摘出来：`passthrough` 是 `argparse.REMAINDER`，它会把
    #   `--secs 300` 一并吞进去当成"转给 cargo test 的参数" ⇒ cargo 报
    #   `Unrecognized option: 'secs'`（2026-10-07 跑 rec_load60 时踩到）。
    argv, secs_cli = _pop_secs(sys.argv[1:])
    a = ap.parse_args(argv)
    if secs_cli is not None:
        a.secs = secs_cli

    names = probe_names()
    if not names:
        print(f"FAIL {PROBE_DIR} 下没有 *.rs")
        return 2

    if a.action == "list":
        print(f"真机探针 {len(names)} 条（需真实硬件，不进门禁，平时不参与编译）：")
        for n in names:
            print(f"  · [{probe_group(n):4}] {n}")
        fast = [n for n in names if probe_group(n) == FAST]
        print(f"\n快组 {len(fast)} 条 / 慢组 {len(names) - len(fast)} 条。"
              f"回归首选：python build/probe.py run fast")
        print("跑某一个：python build/probe.py run <名字>")
        return 0

    if not a.target:
        print("run 需要指定探针名（或 all）")
        return 2

    extra = list(a.passthrough)
    if extra and extra[0] == "--":
        extra = extra[1:]

    # `fast` / `slow` 是组名，不是探针名：按组展开后逐个跑。
    if a.target in (FAST, SLOW):
        group = [n for n in names if probe_group(n) == a.target]
        if not group:
            print(f"{a.target} 组里没有探针")
            return 2
        print(f"== {a.target} 组 {len(group)} 条：{', '.join(group)}")
        rc_all = 0
        for n in group:
            print(f"\n{'=' * 60}\n== {n}\n{'=' * 60}")
            rc_all |= run_probe(n, extra, a.secs)
        print(f"\n{a.target} 组跑完，合计退出码 {rc_all}")
        return rc_all

    if a.target == "all":
        # 逐个跑：每个都要单独登记/还原，且**串行**（录制类探针会抢 DXGI）。
        rc_all = 0
        for n in names:
            print(f"\n{'=' * 60}\n== {n}\n{'=' * 60}")
            rc = run_probe(n, extra, a.secs)
            rc_all |= rc
        print(f"\n全部跑完，合计退出码 {rc_all}")
        return rc_all

    return run_probe(a.target, extra, a.secs)


if __name__ == "__main__":
    sys.exit(main())
