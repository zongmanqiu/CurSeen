"""把 release 产物复制到 build/ 并做 MD5 对账（避免留下旧 exe）。

同时同步 assets（`build.rs` 不拷 assets，漏了会出现"代码改了但面板里选不到新脚本/音效"）。

PowerShell 前台 stdout 在本机取不到，所以结果写文件。
"""

import hashlib
import os
import shutil
import subprocess
import sys
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(TOOLS))
# `.cargo/config.toml` 把 target-dir 设成 "../temp/target"。但 main 可能是 reparse 点，
# 导致 ROOT 的解析在 "main" 有没有被折叠上不稳定——所以两种候选都试，取存在的那个。
def _find_built(name):
    cands = [
        os.path.normpath(os.path.join(ROOT, "temp", "target", "release", name)),
        os.path.normpath(os.path.join(ROOT, "..", "temp", "target", "release", name)),
    ]
    for c in cands:
        if os.path.exists(c):
            return c
    return cands[0]


SRC = _find_built("curseen.exe")
DST = os.path.join(ROOT, "build", "curseen.exe")
OUT = os.path.join(ROOT, "temp", "deploy_report.txt")


def md5(p):
    h = hashlib.md5()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def place_file(src, dst):
    """把 src 放到 dst，返回 (方式, 残留备份路径或 None)。

    目标常常正被一个**提权**的 curseen.exe 当作映像映射着。进程映像句柄带
    FILE_SHARE_DELETE 却**不带** FILE_SHARE_WRITE，于是：
      - `shutil.copy2`（CopyFile2，要 GENERIC_WRITE）→ WinError 32 另一个程序正在使用此文件
      - 但从非提权 shell 里 `taskkill` 杀不掉它（Access denied），`Get-Process` 也拿不到 Path
    实测可行的是**只动目录项**：先写一个全新文件（新建不受旧句柄影响），
    再把旧文件改名让位、新文件改名顶上。运行中的实例继续用旧映像（Windows 保活
    section），新启动的进程拿到新 exe。
    """
    tmp = dst + ".new"
    old = dst + ".old"
    for p in (tmp, old):
        if os.path.exists(p):
            try:
                os.remove(p)      # 上一轮残留（.old 若仍是映像则删不掉，跳过即可）
            except OSError:
                pass
    try:
        shutil.copy2(src, dst)
        return "copy2", (old if os.path.exists(old) else None)
    except PermissionError:
        pass

    shutil.copy2(src, tmp)        # 全新文件，不受旧句柄影响
    if os.path.exists(dst):
        os.rename(dst, old)       # 只依赖 FILE_SHARE_DELETE，实测可行
    os.rename(tmp, dst)
    leftover = None
    if os.path.exists(old):
        try:
            os.remove(old)        # 若仍是映像（delete-pending）会失败，留给系统回收
        except OSError:
            leftover = old
    return "atomic-swap", leftover


lines = []
tk = subprocess.run(["taskkill", "/F", "/IM", "curseen.exe"], capture_output=True, text=True, errors="replace")
lines.append(f"taskkill exit={tk.returncode} out={(tk.stdout or '').strip()} err={(tk.stderr or '').strip()}")
# 非提权 shell 杀不掉提权的实例（Access denied），此时靠 place_file 的原子改名顶上。
if not os.path.exists(SRC):
    lines.append(f"SRC_MISSING {SRC}")
else:
    method, leftover = place_file(SRC, DST)
    lines.append(f"PLACE_METHOD={method}" + (f" leftover={leftover}" if leftover else ""))
    a, b = md5(SRC), md5(DST)
    sa, sb = os.path.getsize(SRC), os.path.getsize(DST)
    lines.append(f"SRC {SRC}\n    size={sa} md5={a} mtime={os.path.getmtime(SRC)}")
    lines.append(f"DST {DST}\n    size={sb} md5={b} mtime={os.path.getmtime(DST)}")
    lines.append(f"MD5_MATCH={a == b}  SIZE_MATCH={sa == sb}")
    # `dir` 的输出是控制台代码页（中文系统 GBK），必须显式 `errors="replace"`：
    # 否则 `text=True` 会在读取线程里抛 UnicodeDecodeError——那行 traceback 会混在
    # 部署日志里，看着像部署失败，实际只是这一句在解码上摔了一跤（2026-09-17 修）。
    _bt = subprocess.run(
        ['cmd', '/C', 'dir /T:W "' + DST + '"'],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    ).stdout.strip()
    lines.append(f"BUILD_TIME={_bt}")

    # PDB 归档：用户侧崩溃的 .dmp 要配**同一次构建**的 PDB 才能精确到行。
    # `temp/target/release/curseen.pdb` 每次重编都被覆盖，若不在这里钉一份，
    # 下次构建后旧 dump 就永远解不开了（Pecia 用 /MAP+固定基址解决同一问题，
    # 本仓保留 ASLR，改用按构建归档 PDB）。放 temp/symbols/（开发侧），不进
    # 分发目录；留最近 KEEP_SYMBOLS 份。
    KEEP_SYMBOLS = 10
    PDB_SRC = _find_built("curseen.pdb")
    SYM_DIR = os.path.join(ROOT, "temp", "symbols")
    if os.path.exists(PDB_SRC):
        os.makedirs(SYM_DIR, exist_ok=True)
        stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(os.path.getmtime(PDB_SRC)))
        sym_dst = os.path.join(SYM_DIR, f"curseen-{stamp}.pdb")
        if not os.path.exists(sym_dst):
            shutil.copy2(PDB_SRC, sym_dst)
        olds = sorted(
            (f for f in os.listdir(SYM_DIR) if f.endswith(".pdb")),
            key=lambda f: os.path.getmtime(os.path.join(SYM_DIR, f)),
            reverse=True,
        )
        for f in olds[KEEP_SYMBOLS:]:
            try:
                os.remove(os.path.join(SYM_DIR, f))
            except OSError:
                pass
        lines.append(f"SYMBOL_ARCHIVED {sym_dst}")
    else:
        lines.append(f"SYMBOL_MISSING {PDB_SRC}")

# 旧录制子进程已废（2026-10-07 收回进程内）：分发目录里不该再出现它。
stale_rec = os.path.join(ROOT, "build", "curseen-rec.exe")
if os.path.exists(stale_rec):
    os.remove(stale_rec)
    lines.append(f"REMOVED_STALE_REC {stale_rec}")
else:
    lines.append("stale curseen-rec.exe 不存在（无需清理）")

# 插桩探针 exe 用完即删，别留在 build/ 里（它默认展开点击波纹，是调试产物）。
probe = os.path.join(ROOT, "build", "curseen_probe.exe")
if os.path.exists(probe):
    os.remove(probe)
    lines.append(f"REMOVED_PROBE {probe}")
else:
    lines.append("probe exe 不存在（无需清理）")

# assets 同步（main/assets → build/assets，只增不改删）。漏这步的典型症状：
# 面板下拉里看不到新加的 .lua / .wav。
sync = os.path.join(TOOLS, "sync_assets.py")
r = subprocess.run(
    [sys.executable, sync], capture_output=True, text=True, encoding="utf-8", errors="replace"
)
lines.append(f"--- sync_assets exit={r.returncode} ---")
report = os.path.join(ROOT, "temp", "assets_sync.txt")
if os.path.exists(report):
    with open(report, encoding="utf-8") as f:
        # 只摘摘要行，逐文件清单太长
        for ln in f.read().splitlines():
            if ln.startswith(("新增", "同步后不一致", "SRC", "DST")):
                lines.append(ln)

# 许可证随包分发：LICENSE + THIRD-PARTY-NOTICES.md + licenses/（各本地 path 依赖
# 的许可原文，自 .3rdParty/ 逐字拷来）。这一步让 build/ 变成**自足的分发目录**，
# 等价于 Pecia 的 `pecia_copy_licenses` CMake ALL 目标。
# 源缺失时 gen_licenses 返回 1 —— 此时 build/ 缺许可声明，不能当成品发出去，
# 所以本脚本也返回 1，让 full.bat 的步骤 6 失败（而不是静默发一个缺许可的包）。
lic = os.path.join(TOOLS, "gen_licenses.py")
r = subprocess.run(
    [sys.executable, lic], capture_output=True, text=True, encoding="utf-8", errors="replace"
)
lines.append(f"--- gen_licenses exit={r.returncode} ---")
rep = os.path.join(ROOT, "temp", "licenses_report.txt")
if os.path.exists(rep):
    with open(rep, encoding="utf-8") as f:
        lines.extend(f.read().splitlines())
lic_ok = r.returncode == 0

with open(OUT, "w", encoding="utf-8") as f:
    f.write("\n".join(lines))
print("\n".join(lines))

# 只有 exe 就位 **且** 许可就位，才算一个能分发的 build/。
sys.exit(0 if lic_ok else 1)
