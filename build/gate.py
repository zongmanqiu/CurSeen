"""按需跑门禁：只验你刚改的那一块，而不是每次都把 745 条全过一遍。

★ 先说清楚**省的是哪一头**（本机实测，2026-09-22）：
  - **745 条测试本身只跑 1.0 秒**（`cargo test --release --lib` 的输出里
    "finished in 1.04s"）。所以"少跑几条测试"省下的是**毫秒级**，不是分钟级。
  - 真正的开销是**编译**：改一个 .rs 后 cargo 要重编整个 crate（149 个文件）
    ≈ **1m12s**。加过滤**挡不住**这笔开销。
  - 于是"按需检验"的真实价值是**信号**（改哪验哪、失败列表短、不会被无关用例的
    噪声淹掉），而不是省时间。
  - 真正省时间的两件事：① `build.rs` 里 `rerun-if-changed` 指向不存在的路径会让
    **每次** cargo 调用都全量重编（已修，零改动从 1m12s 降到 1.7s）；
    ② 改完一批再编一次，不要"改一行编一次"。

用法：
  python build/gate.py                     # 全量（默认）
  python build/gate.py paths key_display   # 只跑这两组
  python build/gate.py --list              # 看有哪些组
  python build/gate.py --check             # 只做类型检查（cargo check --release）
"""

import os
import subprocess
import sys
import time

# 本脚本在 `<项目>/main/build/` 下：上一级就是 crate 根（别写成 dirname(dirname(...))，
# 那样会得到 `main/main`，subprocess 直接 WinError 267 目录名称无效）。
TOOLS = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(TOOLS)

# 组名 → libtest 过滤子串。按**改动点**分组，一组就是一"你刚碰过的东西"。
AREAS = {
    "paths": "core::paths",          # 便携路径 / 目录解析
    "config": "core::config",        # 配置字段、默认值、卡片 id
    "sfx": "ui::sfx",                # 音效目录/候选表/显示
    "keysound": "key_sound",         # 击键音效模块
    "keydisp": "key_display",        # 击键回显（含鼠标按键）
    "vk": "virtual_keyboard",        # 虚拟键盘
    "trail": "trail",                # 鼠标拖尾
    "hover": "hover",                # 悬浮标
    "magnifier": "magnifier",        # 画中画 / 全屏放大
    "record": "recording",           # 录制链路
    "runtime": "runtime::",          # 帧驱动 / 输入门
    "layout": "layout",              # 面板布局
    "script": "script",              # Lua 脚本宿主
}


def run(args, label):
    t0 = time.time()
    r = subprocess.run(args, cwd=CRATE, capture_output=True, text=True,
                       errors="replace", env={**os.environ, "CARGO_INCREMENTAL": "0"})
    dt = time.time() - t0
    ok = r.returncode == 0
    # 只把"跑了几条/结果"抠出来，不把整页输出糊到屏幕上。
    summary = [l.strip() for l in r.stdout.splitlines()
               if l.startswith("test result:") or "panicked" in l or l.startswith("error")]
    print(f"--- {label}  [{dt:.1f}s]  {'OK' if ok else '✗ 失败'} ---")
    for s in summary[:6]:
        print("   " + s)
    if not ok:
        print("   （详细输出见下方 stderr / 重跑时不重定向 stdout）")
        tail = "\n".join(r.stdout.splitlines()[-25:])
        print(tail)
    return ok, dt


def main():
    args = sys.argv[1:]
    if "--list" in args:
        for k, v in AREAS.items():
            print(f"{k:12s} → {v}")
        return 0

    if "--check" in args:
        ok, _ = run(["cargo", "check", "--release"], "cargo check --release")
        return 0 if ok else 1

    picked = [a for a in args if not a.startswith("--")]
    if not picked:
        ok, _ = run(["cargo", "test", "--release", "--lib"], "全量 lib 测试")
        return 0 if ok else 1

    unknown = [p for p in picked if p not in AREAS]
    if unknown:
        print(f"不认识的组：{unknown}；用 --list 看全部")
        return 2

    all_ok = True
    t0 = time.time()
    for p in picked:
        ok, _ = run(["cargo", "test", "--release", "--lib", AREAS[p]], f"{p} ({AREAS[p]})")
        all_ok &= ok
    print(f"=== 合计 {time.time() - t0:.1f}s，{'全绿' if all_ok else '有失败'} ===")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
