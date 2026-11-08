#!/usr/bin/env python3
"""上传后确认宿主更新包**就位**：两个镜像的地址都能取到对应版本的 zip。

判据只有两条（2026-11-09 用户口径，与客户端一致）：**版本号**与**包文件名**。
不比对 sha256、不比对字节数——客户端也不校验，这里再做一遍只会制造"清单与包对不上"
这类只有我们自己关心的失败。

用法（在 main/ 下）：
    python build/check_update_asset.py                       # 探两个镜像的地址
    python build/check_update_asset.py --local ../temp/release/CurSeen_2.1.2.zip

只取每个响应的前 4 个字节（`Range`），不下载整个包：这里要证明的是"地址通、给的是 zip"，
而不是把 3MB 拉两遍。

退出码 0 = 两个镜像都就位；1 = 任一地址不通 / 文件名与版本不符 / 不是 zip。
"""

import argparse
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "update.json"
UA = "CurSeen release check"


def fail(msg: str) -> None:
    print(f"[x] {msg}", file=sys.stderr)
    sys.exit(1)


def zip_file_name(version: str) -> str:
    return f"CurSeen_{version}.zip"


def url_file_name(url: str) -> str:
    tail = url.split("?")[0].split("#")[0].rstrip("/")
    return tail.rsplit("/", 1)[-1]


def looks_like_zip(head: bytes) -> bool:
    return head[:2] == b"PK"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=str(DEFAULT_MANIFEST))
    ap.add_argument("--local", help="改为核对本地 zip，而不是探两个镜像")
    args = ap.parse_args()

    try:
        m = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    except Exception as e:
        fail(f"读清单失败（{args.manifest}）：{e}")
    version = str(m.get("version", ""))
    want = zip_file_name(version)
    urls = m.get("urls", {})

    if args.local:
        p = Path(args.local)
        if not p.is_file():
            fail(f"本地 zip 不存在：{p}")
        if p.name != want:
            fail(f"本地文件名与清单版本不符：{p.name} ≠ {want}")
        if not looks_like_zip(p.open("rb").read(4)):
            fail(f"本地文件不是 zip：{p}")
        print(f"本地核对通过（{version}，{want}）")
        return

    all_ok = True
    for host in ("gitee", "github"):
        url = urls.get(host, "")
        if not url:
            print(f"[x] {host}: 清单没地址")
            all_ok = False
            continue
        if url_file_name(url) != want:
            print(f"[x] {host}: 地址末段不是 {want}：{url}")
            all_ok = False
            continue
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA, "Range": "bytes=0-3"})
            with urllib.request.urlopen(req, timeout=30) as resp:
                head = resp.read(4)
                code = resp.status
        except Exception as e:
            print(f"[x] {host}: 地址不通：{e}")
            all_ok = False
            continue
        if not looks_like_zip(head):
            print(f"[x] {host}: 返回的不是 zip（HTTP {code}）")
            all_ok = False
            continue
        print(f"[ok] {host}: {want}（HTTP {code}）")

    if not all_ok:
        fail("镜像核对未通过：不要宣布发布 / 不要推 update.json")
    print(f"两个镜像都已就位（{version}）")


if __name__ == "__main__":
    main()
