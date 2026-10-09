"""启动冒烟：跑一次 build/curseen.exe，确认能起来且日志里没有 [FATAL]。

用途：改完渲染/运行时之后的第一道闸。GUI 进程没有控制台，panic 只会落进
`<exe 目录>/log/curseen-YYYYMMDD.log`，所以判据只看那个文件的增量。

注意两件事：
  · 程序启动会**提权重启**（`ShellExecuteW(runas)`），真正跑的是提权副本；
    所以"启动器进程已退出"是正常的，不能拿它判断存活。
  · 执行环境会回收派生的子进程，故启、等、杀、读日志必须在**同一个脚本**里跑完。

结果写到 `temp/boot_check.txt`（PowerShell 前台 stdout 在本机取不到）。
"""

import os
import subprocess
import sys
import time

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(TOOLS))
EXE = os.path.join(ROOT, "build", "curseen.exe")
LOG_DIR = os.path.join(ROOT, "build", "log")
OUT = os.path.join(ROOT, "temp", "boot_check.txt")

WAIT_S = 8.0


def today_log():
    return os.path.join(LOG_DIR, time.strftime("curseen-%Y%m%d.log"))


def size_of(p):
    try:
        return os.path.getsize(p)
    except OSError:
        return 0


lines = []

if not os.path.exists(EXE):
    lines.append(f"EXE_MISSING {EXE}")
    with open(OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    print("\n".join(lines))
    sys.exit(2)

# 清场：上次可能还挂着（提权副本不属于本进程的 Job Object，只能按映像名杀）。
subprocess.run(["taskkill", "/F", "/IM", "curseen.exe"], capture_output=True)
time.sleep(0.6)

log = today_log()
before = size_of(log)
lines.append(f"exe   = {EXE}（{size_of(EXE)} B）")
lines.append(f"日志  = {log}，启动前 {before} B")

try:
    subprocess.Popen([EXE], cwd=os.path.dirname(EXE), close_fds=True)
except OSError as e:
    lines.append(f"LAUNCH_FAILED {e}")
else:
    time.sleep(WAIT_S)

# 读增量（提权副本与本进程共用同一个日志文件，按字节偏移取尾部即可）。
after_bytes = b""
with open(log, "rb") as f:
    f.seek(before)
    after_bytes = f.read()
text = after_bytes.decode("utf-8", errors="replace")

alive = subprocess.run(
    ["tasklist", "/FI", "IMAGENAME eq curseen.exe"],
    capture_output=True,
).stdout.decode("utf-8", errors="replace")
running = "curseen.exe" in alive

subprocess.run(["taskkill", "/F", "/IM", "curseen.exe"], capture_output=True)

fatals = [ln for ln in text.splitlines() if "[FATAL]" in ln]
started = "CurSeen v" in text
overlay = "覆盖层" in text

lines.append(f"启动后 {WAIT_S:.0f}s：进程存活={running}，日志增量 {len(after_bytes)} B")
lines.append(f"启动横幅出现 = {started}；覆盖层相关日志出现 = {overlay}")
lines.append(f"[FATAL] 条数 = {len(fatals)}")
for ln in fatals:
    lines.append(f"    {ln}")
lines.append("--- 日志增量 ---")
lines.extend(text.splitlines()[-25:])

ok = started and not fatals
lines.append(f"结果：{'PASS' if ok else 'FAIL'}")

with open(OUT, "w", encoding="utf-8") as f:
    f.write("\n".join(lines))
print("\n".join(lines))
sys.exit(0 if ok else 1)
