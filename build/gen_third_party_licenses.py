"""Generate build/licenses/THIRD-PARTY-LICENSES.txt — the **full licence texts**
of every third-party component that is compiled into the shipped binary.

Why this file exists
--------------------
MIT / Apache-2.0 / BSD-3-Clause all require the copyright notice **and** the
licence text to travel with every distributed copy. "The text is in the cargo
registry cache" does NOT satisfy that: the recipient is not assumed to have a
Rust toolchain. Listing name/version/licence in THIRD-PARTY-NOTICES.md is the
*index*; this file is the *payload* the index points at.

The two local path dependencies (windui / nnnoiseless) keep their own verbatim
files in the same directory (see gen_licenses.py) — those ship upstream files
as-is and are not reproduced here to avoid duplicate copies.

Scope: direct dependencies + the transitive ones THIRD-PARTY-NOTICES.md names
explicitly. The exhaustive tree is `Cargo.lock` (shipped alongside), which is
what the file header says.

Inputs: `main/Cargo.lock` (exact versions) + the cargo registry cache
(`~/.cargo/registry/src/<mirror>/<crate>-<version>/`). Both are present on any
machine that can build this project — compiling requires them.

Idempotent: rewritten only when the bytes differ.
"""

from __future__ import annotations

import glob
import os
import sys
import tomllib

TOOLS = os.path.dirname(os.path.abspath(__file__))
MAIN = os.path.dirname(TOOLS)
ROOT = os.path.dirname(MAIN)

OUT = os.path.join(ROOT, "build", "licenses", "THIRD-PARTY-LICENSES.txt")
LOCK = os.path.join(MAIN, "Cargo.lock")

# (crate, human label, upstream home) — keep in sync with §2 of
# THIRD-PARTY-NOTICES.md. Order = the order in that table.
COMPONENTS = [
    ("windows", "Win32 API 绑定", "https://github.com/microsoft/windows-rs"),
    ("mlua", "Lua 脚本引擎", "https://github.com/mlua-rs/mlua"),
    # ★ Lua 本身没有独立 crate：它的许可随 `lua-src`（mlua vendored 的 Lua 源码）分发，
    #   `mlua-sys` 目录里没有许可文件。
    ("lua-src", "Lua 5.4 源码（mlua vendored）", "https://www.lua.org"),
    ("tiny-skia", "CPU 软渲染光栅化", "https://github.com/linebender/tiny-skia"),
    ("serde", "序列化框架", "https://serde.rs"),
    ("serde_json", "JSON 序列化", "https://github.com/serde-rs/json"),
    ("log", "日志门面", "https://github.com/rust-lang/log"),
    ("env_logger", "日志实现", "https://github.com/rust-cli/env_logger"),
    ("png", "PNG 编解码", "https://github.com/image-rs/image-png"),
    ("gif", "GIF 解码", "https://github.com/image-rs/image-gif"),
    ("ureq", "HTTP 客户端（更新检查）", "https://github.com/algesten/ureq"),
    ("chrono", "日期时间", "https://github.com/chronotope/chrono"),
    ("rand", "随机数", "https://github.com/rust-random/rand"),
    ("toml", "TOML 解析（经 windui）", "https://github.com/toml-rs/toml"),
    ("easyfft", "FFT（经 nnnoiseless）", "https://github.com/nvllz/easyfft"),
    ("once_cell", "一次性初始化（经 nnnoiseless）", "https://github.com/matklad/once_cell"),
]

# File-name patterns that hold a licence text inside a crate source dir.
LIC_GLOBS = ("LICENSE*", "LICENCE*", "COPYING*", "UNLICENSE*")

HEADER = """第三方组件许可全文（THIRD-PARTY-LICENSES.txt）

本文件随 CurSeen 发布包分发，收录**编进 exe 的第三方组件的许可原文**。
MIT / Apache-2.0 / BSD-3-Clause 等许可都要求"副本中保留版权声明与许可全文"，
`THIRD-PARTY-NOTICES.md` 只是索引（组件 / 版本 / 许可 / 上游），**本文件才是那份索引
指向的正文**。

· 范围：直接依赖 + `THIRD-PARTY-NOTICES.md` 中点名的传递依赖；完整依赖树以随包的
  `Cargo.lock` 为准（那里是权威的全集）。
· 两个本地 path 依赖（`windui` / `nnnoiseless`）的原文在本目录各自独立的文件里，
  这里不重复收录。
· CurSeen 自身以 GNU AGPL-3.0 授权，全文见同级的 `LICENSE`。
· 本文件由 `main/build/gen_third_party_licenses.py` 从 `Cargo.lock` + cargo registry
  缓存生成，**不要手改**——改了会在下次部署被覆盖。

════════════════════════════════════════════════════════════════════════════════
"""


def lock_versions() -> dict[str, str]:
    """crate name -> exact version, from Cargo.lock (the authoritative tree)."""
    with open(LOCK, "rb") as f:
        data = tomllib.load(f)
    out: dict[str, str] = {}
    for pkg in data.get("package", []):
        out[pkg["name"]] = pkg["version"]
    return out


def find_crate_dir(name: str, version: str) -> str | None:
    """Locate the extracted crate source in whichever registry mirror has it."""
    for base in glob.glob(os.path.expanduser("~/.cargo/registry/src/*")):
        cand = os.path.join(base, f"{name}-{version}")
        if os.path.isdir(cand):
            return cand
    return None


def licence_files(crate_dir: str) -> list[str]:
    """Every licence-looking file in the crate root (sorted, deduped)."""
    found: list[str] = []
    for pat in LIC_GLOBS:
        found.extend(glob.glob(os.path.join(crate_dir, pat)))
    return sorted(set(p for p in found if os.path.isfile(p)))


def build_text(versions: dict[str, str]) -> tuple[str, list[str]]:
    parts = [HEADER]
    missing: list[str] = []
    for name, label, home in COMPONENTS:
        ver = versions.get(name)
        if ver is None:
            missing.append(f"{name}: Cargo.lock 里没有这个包")
            continue
        cdir = find_crate_dir(name, ver)
        if cdir is None:
            missing.append(f"{name} {ver}: registry 缓存里没有源码目录")
            continue
        files = licence_files(cdir)
        if not files:
            missing.append(f"{name} {ver}: 源码目录里没有许可文件")
            continue
        parts.append(f"\n{'─' * 84}\n{name} {ver}   —   {label}\n{home}\n{'─' * 84}\n")
        for p in files:
            body = open(p, encoding="utf-8", errors="replace").read().strip("\n")
            parts.append(f"\n### {os.path.basename(p)}\n\n{body}\n")
    return "\n".join(parts), missing


def main() -> int:
    if not os.path.exists(LOCK):
        print(f"[MISSING] Cargo.lock: {LOCK}")
        return 1
    versions = lock_versions()
    text, missing = build_text(versions)

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    old = None
    if os.path.exists(OUT):
        old = open(OUT, encoding="utf-8", errors="replace").read()
    if old == text:
        print(f"[ok]      THIRD-PARTY-LICENSES.txt: unchanged ({len(text.encode('utf-8'))} B)")
    else:
        with open(OUT, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        print(f"[WRITE]   THIRD-PARTY-LICENSES.txt: {len(text.encode('utf-8'))} B -> {OUT}")

    for m in missing:
        print(f"[MISSING] {m}")
    print(f"LICENSE_TEXTS_OK={not missing}  dest={OUT}")
    return 0 if not missing else 1


if __name__ == "__main__":
    sys.exit(main())
