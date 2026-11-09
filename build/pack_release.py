#!/usr/bin/env python3
"""生成 CurSeen 发行 zip（**唯一**打包入口，禁止手工压缩 build/）。

# 包内容清单（唯一正本见《开发指南.md》§十）

    curseen.exe
    LICENSE
    licenses/**            ← build/licenses（6_deploy 第 ⑥ 步同步并复核）
    assets/**              ← **main/assets**（源码树），排除 assets/cache/update.json

就这些，**一个不多**。`MANIFEST.txt` 这类打包清单**不进包**——它是给打包的人自己核对的，
不是随包发给用户的文件；本脚本把它写在 zip **旁边**（`<zip>.manifest.txt`），
发布包里出现任何 `MANIFEST.txt` 都会被回读校验判为失败。

运行期数据（`config.json` / `log/` / `temp/` / `data/` / `drafts/` / `*.pdb`）**永不入包**：
内容取自源码树而不是 `build/assets/`——后者是运行期目录，含本机已装脚本与旧文件。

# 为什么用 7-Zip 而不是 Python 的 zipfile

- `-mcu=on` 强制 zip 内的文件名用 **UTF-8** 写（zipfile 走系统 ANSI 时，Windows 自带
  解压器按 GBK 解，中文名会乱码）；
- 找不到 7z 就**直接失败**，不回退 zipfile：两套打包器写出的字节不同，
  "官方包"就没有唯一定义了（排查"用户装的到底是哪个包"时没法对齐）。

# 用法（在 main/ 下）

    python build/pack_release.py --version 2.1.2
    python build/pack_release.py --version 2.1.2 --out-dir ../temp/release

产物两个，都在 `--out-dir`（默认仓库外 `temp/release/`）：

    CurSeen_<版本>.zip              发行包（上传这个）
    CurSeen_<版本>.manifest.txt     打包清单（自己核对用，**不上传**）

7z 的查找顺序：环境变量 `CURSEEN_7Z` → `PATH` 里的 `7z` / `7za` → `%ProgramFiles%\7-Zip\7z.exe`。
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT = ROOT.parent / "temp" / "release"
VERSION_RE = re.compile(r"^\d+(\.\d+)*$")

# 运行期缓存：本机上次拉到的更新清单（版本号可能比本包新/旧），不属于随包内容。
# 客户端联网自己会拉；打进包只会让"这个包到底自称什么版本"出现第二份说法。
SKIP_IN_ASSETS = "cache/update.json"

# 运行期数据 / 调试符号 / 打包清单：**任何**情况下都不许出现在包里（打包后逐条核对）。
FORBIDDEN_SUFFIX = (".pdb",)
FORBIDDEN_NAME = ("config.json", "update.json", "manifest.txt")
FORBIDDEN_DIRS = ("log", "temp", "data", "drafts")


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def find_7z() -> Path:
    """定位 7-Zip 可执行文件。`7zr` 不支持写 zip，故不在候选里。"""
    cands = []
    env = os.environ.get("CURSEEN_7Z")
    if env:
        cands.append(Path(env))
    for name in ("7z", "7za"):
        p = shutil.which(name)
        if p:
            cands.append(Path(p))
    for var in ("ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"):
        base = os.environ.get(var)
        if base:
            cands.append(Path(base) / "7-Zip" / "7z.exe")
    for c in cands:
        if c.is_file():
            return c
    fail(
        "找不到 7-Zip（需要它生成 UTF-8 文件名的 zip）。安装后把 7z 放进 PATH，"
        "或用环境变量 CURSEEN_7Z 指向 7z.exe"
    )
    return Path()  # 不可达，仅为类型收敛


def stage_tree(stage: Path, exe: Path, license_root: Path, licenses: Path, assets: Path) -> None:
    """按白名单把内容摆进 staging 目录（不复制任何运行期数据，也不写清单）。"""
    shutil.copy2(exe, stage / "curseen.exe")
    shutil.copy2(license_root, stage / "LICENSE")
    shutil.copytree(licenses, stage / "licenses")

    def ignore(dirpath, names):
        rel = Path(dirpath).relative_to(assets).as_posix()
        return {n for n in names if f"{rel}/{n}" == SKIP_IN_ASSETS}

    shutil.copytree(assets, stage / "assets", ignore=ignore)


def write_manifest(stage: Path, dest: Path, version: str, seven_ver: str) -> tuple:
    """把逐文件字节数 / 路径写到 **zip 外面**（`<zip>.manifest.txt`）。

    这是打包者自己的核对清单（看"进了哪些文件、各多大"），不随包分发，
    **不记哈希**（2026-11-09 用户口径：更新只认版本号与文件名）。返回 (`行列表`, 文件数)。
    """
    files = sorted(p for p in stage.rglob("*") if p.is_file())
    lines = [
        f"CurSeen {version} 打包清单（打包者自用，不随包分发）",
        f"打包工具: 7-Zip {seven_ver} / zip（-mcu=on，文件名 UTF-8）",
        f"文件数: {len(files)}",
        "",
        f"{'字节':>10}  路径",
    ]
    for p in files:
        rel = p.relative_to(stage).as_posix()
        lines.append(f"{p.stat().st_size:>10}  {rel}")
    dest.write_text("\r\n".join(lines) + "\r\n", encoding="utf-8")
    return lines, len(files)


def seven_zip_version(seven: Path) -> str:
    try:
        out = subprocess.run(
            [str(seven)], capture_output=True, text=True, errors="replace"
        ).stdout
    except OSError as e:
        fail(f"7-Zip 无法执行（{seven}）：{e}")
        return ""
    m = re.search(r"7-Zip[^\n]*?\s(\d+\.\d+)", out)
    return m.group(1) if m else "?"


def make_zip(seven: Path, stage: Path, zip_path: Path) -> None:
    if zip_path.exists():
        zip_path.unlink()
    # `.` 作为输入：从 staging 里取，包内路径就是 staging 的相对路径。
    cmd = [
        str(seven),
        "a",
        "-tzip",
        "-mx=9",
        "-mcu=on",
        "-bso0",
        "-bsp0",
        str(zip_path),
        ".",
    ]
    res = subprocess.run(cmd, cwd=stage, capture_output=True, text=True, errors="replace")
    if res.returncode != 0:
        fail(f"7-Zip 打包失败（exit={res.returncode}）：{res.stderr.strip() or res.stdout.strip()}")


def verify_zip(zip_path: Path, stage: Path) -> list:
    """回读校验：文件集合必须与 staging **完全一致**，且不许有运行期文件或打包清单。"""
    want = {
        p.relative_to(stage).as_posix() for p in stage.rglob("*") if p.is_file()
    }
    with zipfile.ZipFile(zip_path) as z:
        infos = z.infolist()
        got = {i.filename for i in infos if not i.is_dir()}
        missing = sorted(want - got)
        extra = sorted(got - want)
        if missing:
            fail(f"包内缺少文件：{missing[:5]}（共 {len(missing)} 个）")
        if extra:
            fail(f"包内多出文件：{extra[:5]}（共 {len(extra)} 个）")

        bad = []
        for name in sorted(got):
            parts = name.split("/")
            if any(d in FORBIDDEN_DIRS for d in parts[:-1]):
                bad.append(f"{name}（运行期目录）")
            elif name.endswith(FORBIDDEN_SUFFIX):
                bad.append(f"{name}（调试符号）")
            elif parts[-1].lower() in FORBIDDEN_NAME:
                bad.append(f"{name}（运行期/自指/打包清单）")
        if bad:
            fail("包里混进了不该发布的文件：" + "；".join(bad[:5]))

        # 非 ASCII 名必须带 UTF-8 标志位（0x800），否则 Windows 解压会按 GBK 解。
        garbled = [
            i.filename
            for i in infos
            if not i.is_dir() and not i.filename.isascii() and not (i.flag_bits & 0x800)
        ]
        if garbled:
            fail(f"包内中文文件名没用 UTF-8 编码：{garbled[:3]}")
    return sorted(got)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="版本号，例如 2.1.2")
    ap.add_argument("--out-dir", default=str(DEFAULT_OUT), help="zip 输出目录（仓库外）")
    args = ap.parse_args()
    version = args.version.strip()
    if not VERSION_RE.match(version):
        fail(f"版本号不合法：{version}")

    build = ROOT.parent / "build"
    exe = build / "curseen.exe"
    licenses = build / "licenses"
    assets = ROOT / "assets"
    license_root = ROOT / "LICENSE"
    for p, what in [
        (exe, "build/curseen.exe（先跑 6_deploy）"),
        (licenses, "build/licenses（先跑 6_deploy）"),
        (assets, "main/assets"),
        (license_root, "main/LICENSE"),
    ]:
        if not p.exists():
            fail(f"缺少 {what}：{p}")

    seven = find_7z()
    seven_ver = seven_zip_version(seven)

    # ★ 必须先 resolve 成绝对路径：`make_zip` 是 `cwd=stage` 调 7z 的，
    #   传进去的相对路径会被按 staging 解释，产出落到 `<out>/<out>/` 里，
    #   然后 verify_zip 找不到包 → 报一个和真因无关的 FileNotFoundError。
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    zip_path = out_dir / f"CurSeen_{version}.zip"
    manifest_path = out_dir / f"CurSeen_{version}.manifest.txt"
    stage = out_dir / f"_stage_{version}"
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)

    try:
        stage_tree(stage, exe, license_root, licenses, assets)
        lines, count = write_manifest(stage, manifest_path, version, seven_ver)
        make_zip(seven, stage, zip_path)
        names = verify_zip(zip_path, stage)
    finally:
        shutil.rmtree(stage, ignore_errors=True)

    print(f"已生成 {zip_path}")
    print(f"打包器={seven} （7-Zip {seven_ver}，-mcu=on）")
    print(f"文件数={count}")
    print(f"size={zip_path.stat().st_size}")
    print(f"打包清单={manifest_path}（打包者自用，不会进 zip，也不上传）")
    print("清单前 6 行：")
    for line in lines[:6]:
        print("  " + line)
    print(f"校验：{count} 个文件与清单一一对应；无运行期数据、无 .pdb、无打包清单、中文名 UTF-8 ✓")
    print("下一步：上传 Gitee / GitHub release（tag 与文件名同名；只传 zip，不传 manifest），再跑")
    print("        python build/gen_update_json.py --version <版本> --zip <本文件>")
    print("        python build/check_update_asset.py（上传后核对两个镜像）")
    _ = names


if __name__ == "__main__":
    main()
