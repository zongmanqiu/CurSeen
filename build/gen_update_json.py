#!/usr/bin/env python3
"""生成宿主更新清单 update.json（版本 / HISTORY 当前段 / sha256 / 双镜像 zip 地址）。

用法（在 main/ 下）：
    python build/gen_update_json.py --version 2.1.0 --zip ../build/release/CurSeen_2.1.0.zip
    python build/gen_update_json.py --version 2.1.0 --zip ... --out ../update.json

默认输出到 main/update.json；把生成物提交到宿主仓库 main 根目录即可。
`notes` 只取 HISTORY.md 里 `## <version>`（允许 `（日期）` 后缀）到下一个 `## ` 或 `---` 之前，
不把整份 HISTORY 塞进客户端弹窗。
"""

import argparse
import hashlib
import json
import re
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HISTORY = ROOT / "HISTORY.md"
VERSION_RE = re.compile(r"^\d+(\.\d+)*$")


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def history_notes(version: str) -> str:
    text = HISTORY.read_text(encoding="utf-8")
    # `## 2.1.0（2026-10-30）` 或 `## 2.1.0`；下一节标题或分隔线为止。
    pat = re.compile(
        rf"(?ms)^## {re.escape(version)}(?:\s*（[^）]*）)?\s*\n(.*?)(?=^## |^---\s*$|\Z)"
    )
    m = pat.search(text)
    if not m:
        fail(f"HISTORY.md 缺少 `## {version}` 段（先按发布清单压缩本版历史）")
    notes = m.group(1).strip()
    if not notes:
        fail(f"HISTORY.md 的 `## {version}` 段是空的")
    return notes


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="发布版本号，如 2.1.0")
    ap.add_argument("--zip", required=True, help="对应的 CurSeen_<version>.zip")
    ap.add_argument(
        "--published",
        default=date.today().isoformat(),
        help="发布日期（默认今天，ISO 形式）",
    )
    ap.add_argument("--out", default=str(ROOT / "update.json"), help="输出路径")
    args = ap.parse_args()

    version = args.version.strip()
    if not VERSION_RE.match(version):
        fail(f"版本号不合法：{version}")
    zip_path = Path(args.zip)
    if not zip_path.is_file():
        fail(f"zip 不存在：{zip_path}")
    tag = version
    data = {
        "version": version,
        "published": args.published,
        "notes": history_notes(version),
        "sha256": sha256_file(zip_path),
        "urls": {
            "gitee": f"https://gitee.com/qiuzongman/curseen/releases/download/{tag}/CurSeen_{version}.zip",
            "github": f"https://github.com/zongmanqiu/CurSeen/releases/download/{tag}/CurSeen_{version}.zip",
        },
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"已生成 {out}（{version}，{zip_path.stat().st_size} B）")
    print(f"sha256={data['sha256']}")


if __name__ == "__main__":
    main()
