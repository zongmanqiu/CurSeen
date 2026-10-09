"""分层依赖体检（只读）——对齐 AGENTS.md 第三节的强制分层。

用法：
  python build/layering_scan.py            # 常规输出：违规清单 + 汇总
  python build/layering_scan.py --json     # 机器可读：违规明细 JSON

分层（低 → 高）：core → platform → scripting → modules → update → app → ui → runtime
规则：
  1. 高层可知道低层，低层**不得**知道高层（`use crate::<目标>` 的目标层 > 本层 → 违规）。
  2. `core` 与 `modules` **禁用** `windui` / `windows` crate（可测性的根基）。
退出码：0 = 「违规 0 处」；1 = 有违规。

本脚本只做静态字符串扫描，不引第三方正则库；判断的是 `use crate::` 直引。
"""
import json
import pathlib
import sys

# 层名 → 层序（数字越大越高层）。与《目录结构说明.md》第三节一一对应。
LAYER_ORDER = {
    "core": 0,
    "platform": 1,
    "scripting": 2,
    "modules": 3,
    "update": 4,
    "app": 5,
    "ui": 6,
    "runtime": 7,
}

# 这些顶层模块允许被 `use crate::` 引用（lib.rs / main.rs 的装配代码，不判层）。
# 其它一律按 LAYER_ORDER 判。
ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent  # <main>/src 的父级
SRC = ROOT_DIR / "src"

# core 与 modules 禁用的外部 crate（裸调用，可测试性红线）。
BARRED_CRATES = {"windui", "windows"}
BARRED_LAYERS = {0, 3}  # core=0, modules=3


def file_layer(rel: str):
    """取 `src/<X>/.../f.rs` 的第一个路径段 → 层名 → 层序。非分层文件返回 None。"""
    parts = rel.replace("\\", "/").split("/")
    if not parts:
        return None
    name = parts[0]
    idx = LAYER_ORDER.get(name)
    return idx if idx is not None else None


def iter_rs():
    for p in sorted(SRC.rglob("*.rs")):
        yield p


def scan_uses(body: str):
    """抓所有 `use crate::<X>` 的目标模块名（同一行可多段，兼容 `use crate::{a, b}`）。"""
    out = []
    for line in body.splitlines():
        s = line.strip()
        if not s.startswith("use crate::"):
            continue
        rest = s[len("use crate::"):]
        # 掐掉行尾分号
        rest = rest.split(";")[0].split(" as ")[0].strip()
        # `use crate::{a, b}` vs `use crate::a::…`
        if rest.startswith("{"):
            rest = rest[1:]
        if "}" in rest:
            rest = rest.split("}")[0]
        for item in rest.split(","):
            item = item.strip()
            if not item:
                continue
            name = item.split("::")[0]
            if name:
                out.append(name)
    return out


def main():
    violations = []
    for p in iter_rs():
        rel = p.relative_to(SRC.parent).as_posix()
        fl = file_layer(str(p.relative_to(SRC)))
        if fl is None:
            continue  # src 根下的单文件（如 lib.rs 分支）不判
        body = p.read_text(encoding="utf-8", errors="replace")
        for target in scan_uses(body):
            t_idx = LAYER_ORDER.get(target)
            if t_idx is None:
                continue  # 非顶层模块 crate（如 crate::core::x 的 core 已覆盖）
            if t_idx > fl:
                violations.append(
                    f"{rel}: `use crate::{target}` —— 低层({list(LAYER_ORDER)[fl]})"
                    f"不得引用高层({target})"
                )
        # core 与 modules 禁引外部 crate
        if fl in BARRED_LAYERS:
            for word in BARRED_CRATES:
                if f"use {word}::" in body or f"use {word};" in body:
                    violations.append(
                        f"{rel}: {list(LAYER_ORDER)[fl]} 层禁引外部 crate `{word}`"
                    )

    if "--json" in sys.argv:
        print(json.dumps({"violations": violations, "count": len(violations)},
                         ensure_ascii=False, indent=2))
    else:
        if violations:
            for v in violations:
                print(" [" + "✗".center(1) + "] " + v)
            print(f"\n违规 {len(violations)} 处")
            return 1
        print("违规 0 处")
    return 0


if __name__ == "__main__":
    sys.exit(main())