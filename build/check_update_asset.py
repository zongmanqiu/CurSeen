#!/usr/bin/env python3
"""上传后核对宿主更新包：两个镜像下载到的 zip 必须与 update.json 的 sha256 完全一致。

为什么需要它（2026-10-31 事故）：本地 zip 被手工重打过一次，`update.json` 里的哈希仍
指向旧文件；用户点更新拿到“下载失败：sha256 不匹配”。校验没有误报，是发布物与清单不是
同一个文件。本脚本把“上传后的真实字节”作为判据。

用法（在 main/ 下）：
    python build/check_update_asset.py                    # 下载 update.json 里的两个地址
    python build/check_update_asset.py --local ../temp/release/CurSeen_2.1.1.zip
                                                          # 上传前先核对本地文件

退出码 0 = 全部一致；1 = 任一项不一致/网络失败/不是 zip。
"""

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "update.json"
MAX_BYTES = 128 * 1024 * 1024
UA = "CurSeen release check"


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def check_bytes(name: str, data: bytes, expected: str) -> bool:
    actual = hashlib.sha256(data).hexdigest()
    if len(data) < 4 or data[:2] != b"PK":
        print(f"[x] {name}: 返回的不是 zip（{len(data)} B）")
        return False
    if actual != expected:
        print(f"[x] {name}: sha256 不匹配\n    期望 {expected}\n    实际 {actual}（{len(data)} B）")
        return False
    print(f"[ok] {name}: sha256={actual}（{len(data)} B）")
    return True


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=str(DEFAULT_MANIFEST))
    ap.add_argument("--local", help="改为核对本地 zip，而不是下载两个镜像")
    args = ap.parse_args()

    manifest = Path(args.manifest)
    try:
        m = json.loads(manifest.read_text(encoding="utf-8"))
    except Exception as e:
        fail(f"读清单失败（{manifest}）：{e}")
    expected = str(m.get("sha256", "")).lower()
    if len(expected) != 64:
        fail("update.json 的 sha256 不是 64 位十六进制")
    urls = m.get("urls", {})
    version = m.get("version", "?")

    if args.local:
        p = Path(args.local)
        if not p.is_file():
            fail(f"本地 zip 不存在：{p}")
        ok = check_bytes(str(p), p.read_bytes(), expected)
        if not ok:
            fail("本地文件与 update.json 不一致——先重跑 gen_update_json")
        print(f"本地核对通过（{version}）")
        return

    all_ok = True
    for host in ("gitee", "github"):
        url = urls.get(host, "")
        if not url:
            print(f"[x] {host}: 清单没地址")
            all_ok = False
            continue
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=30) as resp:
                data = resp.read(MAX_BYTES + 1)
        except Exception as e:
            print(f"[x] {host}: 下载失败：{e}")
            all_ok = False
            continue
        if len(data) > MAX_BYTES:
            print(f"[x] {host}: 响应超过 {MAX_BYTES} B")
            all_ok = False
            continue
        all_ok = check_bytes(host, data, expected) and all_ok

    if not all_ok:
        fail("镜像核对未通过：不要宣布发布 / 不要推 update.json")
    print(f"两个镜像都与 update.json 一致（{version}）")


if __name__ == "__main__":
    main()
