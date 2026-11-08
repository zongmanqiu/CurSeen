#!/usr/bin/env python3
"""生成宿主更新清单 update.json（版本 / HISTORY 当前段 / 双镜像 zip 地址）。

清单里**不记哈希**（2026-11-09 用户口径）：更新只校验**版本号**与**包文件名**，
下载到的 zip 不做字节级校验。所以这里除了版本号，唯一有约束力的就是地址末段必须
等于 `CurSeen_<版本>.zip`。

用法（在 main/ 下）：
    python build/gen_update_json.py --version 2.1.0 --zip ../build/release/CurSeen_2.1.0.zip
    python build/gen_update_json.py --version 2.1.0 --zip ... --out ../update.json

`--zip` 只用来确认"这个版本的包确实打好了"（存在即可），不参与任何计算。
默认输出到 main/update.json；把生成物提交到宿主仓库 main 根目录即可。
`notes` 只取 HISTORY.md 里 `## <version>`（允许 `（日期）` 后缀）到下一个 `## ` 或 `---` 之前，
不把整份 HISTORY 塞进客户端弹窗。
"""

import argparse
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


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="发布版本号，如 2.1.0")
    ap.add_argument("--zip", required=True, help="对应的 CurSeen_<version>.zip（只确认存在）")
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
    name = f"CurSeen_{version}.zip"
    data = {
        "version": version,
        "published": args.published,
        "notes": history_notes(version),
        "urls": {
            "gitee": f"https://gitee.com/qiuzongman/curseen/releases/download/{version}/{name}",
            "github": f"https://github.com/zongmanqiu/CurSeen/releases/download/{version}/{name}",
        },
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"已生成 {out}（{version}，{name}）")


if __name__ == "__main__":
    main()
