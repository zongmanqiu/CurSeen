#!/usr/bin/env python3
"""生成 CurSeen 发行 zip（唯一入口，禁止手工压缩 build/）。

产物结构与 `CurSeen_<version>.zip` 一致：
    curseen.exe
    LICENSE
    licenses/...
    assets/...            ← 来自 **main/assets**（源码树，干净）
                            **排除 assets/docs/update.json**（清单含 zip 自身哈希，不能自指）

为什么必须用它：
- 手工压缩会把 `build/assets/docs/update.json` 带进包（自指），并用系统 ANSI/GBK 写文件名，
  与 UTF-8 包名不一致；
- 发行清单里的 sha256 必须对应**这个脚本产出的文件**，上传后由
  `build/check_update_asset.py` 再从两个镜像下载核对。

用法（在 main/ 下）：
    python build/pack_release.py --version 2.1.1
    python build/pack_release.py --version 2.1.1 --out-dir ../temp/release
"""

import argparse
import hashlib
import os
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT = ROOT.parent / "temp" / "release"
VERSION_RE = re.compile(r"^\d+(\.\d+)*$")


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="版本号，例如 2.1.1")
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

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    zip_path = out_dir / f"CurSeen_{version}.zip"
    if zip_path.exists():
        zip_path.unlink()

    # 排除自指清单；其余 assets 原样带上（含 scripts.json 离线目录）。
    skip = {"assets/docs/update.json"}

    def add_tree(z: zipfile.ZipFile, src: Path, prefix: str) -> None:
        for dirpath, _dirnames, filenames in os.walk(src):
            rel_dir = os.path.relpath(dirpath, src).replace(os.sep, "/")
            if rel_dir != ".":
                arc_dir = f"{prefix}/{rel_dir}/"
                z.writestr(zipfile.ZipInfo(arc_dir), b"")
            for name in filenames:
                rel = f"{rel_dir}/{name}" if rel_dir != "." else name
                arc = f"{prefix}/{rel}" if prefix else rel
                if arc in skip:
                    continue
                z.write(os.path.join(dirpath, name), arc)

    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.write(exe, "curseen.exe")
        z.write(license_root, "LICENSE")
        add_tree(z, licenses, "licenses")
        add_tree(z, assets, "assets")

    digest = hashlib.sha256(zip_path.read_bytes()).hexdigest()
    size = zip_path.stat().st_size
    print(f"已生成 {zip_path}")
    print(f"size={size}")
    print(f"sha256={digest}")
    print("下一步：上传到 Gitee / GitHub release（tag 与文件名同名），再跑")
    print("        python build/gen_update_json.py --version <版本> --zip <本文件>")
    print("        python build/check_update_asset.py（上传后核对两个镜像）")


if __name__ == "__main__":
    main()
