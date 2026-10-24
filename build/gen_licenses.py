"""Assemble the licence bundle that ships inside <project>\\build\\.

<project>\\build\\ is a *distributable* directory (exe + assets + config). For
licence compliance it must also be self-contained:

    LICENSE                 <- main\\LICENSE                    (verbatim)
    licenses\\THIRD-PARTY-NOTICES.md
                            <- main\\THIRD-PARTY-NOTICES.md     (verbatim)
    licenses\\              <- one verbatim text per LOCAL (path) dependency,
                               copied straight out of .3rdParty\\<pkg>\\

This is the CurSeen equivalent of Pecia's `pecia_copy_licenses` CMake target
(an ALL target that runs on every build and uses copy_if_different so it is
idempotent). Step 6 (deploy) calls it, which is why `build\\` needs no manual
touch-up before shipping.

Notes
-----
* The crates.io dependencies are not bundled as one-file-per-crate: their full
  licence texts are concatenated into `licenses/THIRD-PARTY-LICENSES.txt` by
  `gen_third_party_licenses.py` (called from here, so deploy needs no extra
  step). They are listed by name/version/licence in THIRD-PARTY-NOTICES.md.
  Both are required: MIT/Apache-2.0/BSD need the *text* to travel with the copy,
  and "it lives in the cargo registry cache" does not hold for a recipient
  without a Rust toolchain.
* Source names differ upstream (windui ships LICENSE-MIT + LICENSE-APACHE),
  so the mapping is explicit below rather than guessed by glob - same lesson
  Pecia recorded ("注意上游文件名不统一").
* Idempotent: a file is only rewritten when its bytes differ.
* Exit code is 1 when a source is missing, so full.bat's step 6 fails loudly
  instead of silently shipping a build\\ without licences. (On a clean
  checkout this cannot happen: step 2 populates .3rdParty\\ first.)
"""

import os
import shutil
import subprocess
import sys

TOOLS = os.path.dirname(os.path.abspath(__file__))
MAIN = os.path.dirname(TOOLS)
ROOT = os.path.dirname(MAIN)

DEST = os.path.join(ROOT, "build")
DEST_LIC = os.path.join(DEST, "licenses")
REPORT = os.path.join(ROOT, "temp", "licenses_report.txt")

# Files copied verbatim out of main\ into build\.
# LICENSE sits at build\ root (the "what am I licensed under" entry point);
# NOTICES groups with the other third-party licence files in build\licenses\.
MAIN_FILES = [
    ("LICENSE", "本程序 AGPL-3.0 全文", True),
    ("THIRD-PARTY-NOTICES.md", "第三方组件清单 / 修改说明 / 分发义务", False),
]

# dest-name inside build\licenses\ <- source relative to the project root.
# Keep the dest name "<pkg>-<upstream-name>.txt" so it is obvious where it came
# from even after .3rdParty\ is gone.
# (nnnoiseless used to be here too; since 2026-09-28 it is a plain crates.io
#  dependency, so its COPYING travels via THIRD-PARTY-LICENSES.txt instead.)
LICENSE_BUNDLE = [
    ("windui-LICENSE-MIT.txt", ".3rdParty/wind-ui-rust-0.30.0/LICENSE-MIT", "windui 0.30.0 (MIT)"),
    ("windui-LICENSE-APACHE.txt", ".3rdParty/wind-ui-rust-0.30.0/LICENSE-APACHE", "windui 0.30.0 (Apache-2.0)"),
    # 出厂声音素材（assets/Sounds/*.wav，Kenney UI Audio）的 CC0 原文。
    # CC0 本无署名义务，但随包带上全文是最干净的姿势（2026-09-29 用户要求）。
    ("CC0-1.0.txt", "main/licenses/CC0-1.0.txt", "Kenney UI Audio (CC0 1.0)"),
]


def _same(a, b):
    """Byte-identical?"""
    if not (os.path.exists(a) and os.path.exists(b)):
        return False
    if os.path.getsize(a) != os.path.getsize(b):
        return False
    with open(a, "rb") as fa, open(b, "rb") as fb:
        while True:
            x, y = fa.read(1 << 16), fb.read(1 << 16)
            if x != y:
                return False
            if not x:
                return True


def place(src, dst, lines, label):
    """copy_if_different; returns True on success, False when src is missing."""
    if not os.path.exists(src):
        lines.append(f"[MISSING] {label}: no source {src}")
        return False
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    if _same(src, dst):
        lines.append(f"[ok]      {label}: unchanged ({os.path.getsize(src)} B)")
    else:
        shutil.copy2(src, dst)
        lines.append(f"[COPY]    {label}: {os.path.getsize(src)} B -> {dst}")
    return True


def main():
    lines = []
    ok = True

    # 1. our own licence + the notices file, straight out of main\
    for name, label, at_root in MAIN_FILES:
        target = os.path.join(DEST if at_root else DEST_LIC, name)
        ok &= place(os.path.join(MAIN, name), target, lines, name + "  " + label)

    # 2. one verbatim text per local (path) dependency
    for dest_name, rel_src, label in LICENSE_BUNDLE:
        ok &= place(os.path.join(ROOT, rel_src), os.path.join(DEST_LIC, dest_name), lines, dest_name + "  " + label)

    # 3. the full licence texts of every crates.io component compiled into the exe.
    #    MIT / Apache-2.0 / BSD all require the text to travel with the copy —
    #    "it is in the cargo registry cache" does not satisfy that for a
    #    recipient without a Rust toolchain. THIRD-PARTY-NOTICES.md is the index,
    #    THIRD-PARTY-LICENSES.txt is the payload it points at.
    rc = subprocess.run(
        [sys.executable, os.path.join(TOOLS, "gen_third_party_licenses.py")],
        cwd=ROOT,
    ).returncode
    ok &= rc == 0
    lines.append(f"[{'ok' if rc == 0 else 'FAIL'}]      THIRD-PARTY-LICENSES.txt: gen_third_party_licenses exit={rc}")

    summary = f"LICENSES_OK={ok}  dest={DEST}"
    lines.append(summary)

    os.makedirs(os.path.dirname(REPORT), exist_ok=True)
    with open(REPORT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
