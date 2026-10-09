"""把 main/assets/ 单向同步到 build/assets/（只增不改删）。

**为什么需要它**：`build.rs` 不拷 assets，`cargo build` 之后 `build/assets/` 仍是旧的
——新增/改名的脚本、声音不会出现在面板下拉里，表现为"代码明明改了，UI 里选不到"。

**为什么是"只增不改删"而不是镜像**：`build/assets/` 是**运行期数据目录**，用户会在里面
放自己的东西（导入的悬浮标图片、自写脚本、自备音效）。镜像式同步会把它们当"多余文件"删掉，
直接毁掉用户数据。所以这里只做：目标缺 → 拷；源更新（mtime/size 不同）→ 覆盖。
目标多出来的文件一律保留。

用法：
    python sync_assets.py            # 同步并写报告
    python sync_assets.py --dry-run  # 只看会做什么，不动文件
"""

import filecmp
import os
import shutil
import sys

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(TOOLS))
SRC_ROOT = os.path.join(ROOT, "main", "assets")
DST_ROOT = os.path.join(ROOT, "build", "assets")
OUT = os.path.join(ROOT, "temp", "assets_sync.txt")


def walk(root):
    """返回 {相对路径: (大小, mtime)}。"""
    found = {}
    for dirpath, _dirnames, filenames in os.walk(root):
        for name in filenames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root)
            st = os.stat(full)
            found[rel] = (st.st_size, int(st.st_mtime))
    return found


def main(dry_run=False):
    lines = []
    if not os.path.isdir(SRC_ROOT):
        lines.append(f"SRC_MISSING {SRC_ROOT}")
        with open(OUT, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
        return 1

    added, updated, same = [], [], []
    for rel, (size, _mtime) in sorted(walk(SRC_ROOT).items()):
        src = os.path.join(SRC_ROOT, rel)
        dst = os.path.join(DST_ROOT, rel)
        if not os.path.exists(dst):
            added.append(rel)
        elif not filecmp.cmp(src, dst, shallow=False):
            updated.append(rel)
        else:
            same.append(rel)
        if rel in added or rel in updated:
            if not dry_run:
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                shutil.copy2(src, dst)

    # 目标侧多出来的文件：**故意保留**，逐条列出来让用户知道它们还在。
    extra = []
    if os.path.isdir(DST_ROOT):
        for rel in sorted(walk(DST_ROOT)):
            if not os.path.exists(os.path.join(SRC_ROOT, rel)):
                extra.append(rel)

    lines.append(f"SRC {SRC_ROOT}")
    lines.append(f"DST {DST_ROOT}")
    lines.append(f"dry_run={dry_run}")
    lines.append(f"新增 {len(added)} / 覆盖 {len(updated)} / 一致 {len(same)} / 目标独有 {len(extra)}")
    for tag, items in (("+ 新增", added), ("~ 覆盖", updated), ("= 保留(目标独有)", extra)):
        if items:
            lines.append(f"--- {tag} ---")
            lines.extend(f"    {r}" for r in items)

    # 同步后复核：源里每个文件都必须在目标里字节相同。
    still_bad = []
    if not dry_run:
        for rel in walk(SRC_ROOT):
            src = os.path.join(SRC_ROOT, rel)
            dst = os.path.join(DST_ROOT, rel)
            if not os.path.exists(dst) or not filecmp.cmp(src, dst, shallow=False):
                still_bad.append(rel)
    lines.append(f"同步后不一致 = {still_bad if still_bad else '0（全部一致）'}")

    with open(OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    print("\n".join(lines))
    return 0 if not still_bad else 1


if __name__ == "__main__":
    sys.exit(main(dry_run="--dry-run" in sys.argv))
