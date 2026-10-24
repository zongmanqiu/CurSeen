# -*- coding: utf-8 -*-
"""长测联动驱动（单进程跑完全程：起 exe、开特效、合成输入、等探针、改名产物）。

设计要点（见当日踩坑/记忆）：
- ★ 执行环境会回收派生子进程 ⇒ 起 GUI、驱动、收尾**必须在同一个脚本里跑完**。
- ★ 合成输入会持续重置系统空闲计时器，另外每 60s 调一次 SetThreadExecutionState
  双保险 —— 熄屏/锁屏会让桌面复制 ACCESS_LOST，2h 测试死在半路。
- ★ 打字进记事本沙箱（强杀退出，不保存），绝不往用户的编辑器里打字。
- ★ 音效类功能不开（半夜吵人）；声音通路由 soak_link 探针内部的 WASAPI 管线覆盖。
- ★ 探针跑完把产物改名移到 <ROOT>/长测产物/，用户早上直接看。

用法：
  python build/soak_drive.py --dry      # 预演 60 秒 + 截图，不起探针
  python build/soak_drive.py            # 全量：7200s 探针 + 驱动 + 改名
"""
from __future__ import annotations

import ctypes
import json
import math
import os
import random
import shutil
import subprocess
import sys
import time
from ctypes import wintypes
from datetime import datetime

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BUILD = os.path.join(ROOT, "build")
MAIN = os.path.join(ROOT, "main")
OUTDIR = os.path.join(ROOT, "长测产物")
PROD_SRC = os.path.join(MAIN, "..", "temp", "soaklink")
CONFIG = os.path.join(BUILD, "config.json")
CONFIG_BAK = os.path.join(BUILD, "config.json.bak_soak")

u32 = ctypes.WinDLL("user32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)

# ───────────────────────── Win32 输入 ─────────────────────────
INPUT_MOUSE, INPUT_KEYBOARD = 0, 1
KEYEVENTF_KEYUP = 0x0002
LEFTDOWN, LEFTUP, RIGHTDOWN, RIGHTUP = 0x0002, 0x0004, 0x0008, 0x0010
MOUSEEVENTF_MOVE_ABS = 0x0001 | 0x8000  # MOVE | ABSOLUTE
VK_SHIFT, VK_BACK, VK_RETURN, VK_SPACE = 0x10, 0x08, 0x0D, 0x20
ES_CONTINUOUS, ES_SYSTEM_REQUIRED, ES_DISPLAY_REQUIRED = (
    0x80000000,
    0x00000001,
    0x00000002,
)


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wintypes.LONG),
        ("dy", wintypes.LONG),
        ("mouseData", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(wintypes.ULONG)),
    ]

class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wintypes.WORD),
        ("wScan", wintypes.WORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(wintypes.ULONG)),
    ]


class _U(ctypes.Union):
    _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("u", _U)]


def _send(*items: INPUT) -> None:
    arr = (INPUT * len(items))(*items)
    if not u32.SendInput(len(arr), arr, ctypes.sizeof(INPUT)):
        raise OSError(f"SendInput 失败: {ctypes.get_last_error()}")


def _ki(vk: int, flags: int) -> INPUT:
    i = INPUT()
    i.type = INPUT_KEYBOARD
    i.u.ki = KEYBDINPUT(vk, 0, flags, 0, None)
    return i


def _mi(flags: int) -> INPUT:
    i = INPUT()
    i.type = INPUT_MOUSE
    i.u.mi = MOUSEINPUT(0, 0, 0, flags, 0, None)
    return i


def tap(vk: int) -> None:
    _send(_ki(vk, 0))
    time.sleep(0.02)
    _send(_ki(vk, KEYEVENTF_KEYUP))


def type_char(ch: str) -> None:
    if ch == " ":
        tap(VK_SPACE)
    elif ch == "\n":
        tap(VK_RETURN)
    else:
        vk = ord(ch.upper())
        shift = ch.isupper()
        if shift:
            _send(_ki(VK_SHIFT, 0))
        tap(vk)
        if shift:
            _send(_ki(VK_SHIFT, KEYEVENTF_KEYUP))


def click(x: int, y: int, right: bool = False) -> None:
    u32.SetCursorPos(int(x), int(y))
    time.sleep(0.04)
    down, up = (RIGHTDOWN, RIGHTUP) if right else (LEFTDOWN, LEFTUP)
    _send(_mi(down))
    time.sleep(0.03)
    _send(_mi(up))


def move_abs(x: int, y: int, sw: int, sh: int) -> None:
    """SendInput 绝对移动。

    ★★★ 必须走 SendInput 而不是 SetCursorPos：拖尾靠**低级鼠标钩子**
    （WH_MOUSE_LL）采样轨迹，`SetCursorPos` 是光标重定位、不产生 LL 钩子事件
    （预演实测：悬浮标在动、拖尾一笔都没画）。绝对坐标归一化到 0..65535。
    """
    nx = int(x * 65535 / max(sw - 1, 1))
    ny = int(y * 65535 / max(sh - 1, 1))
    i = INPUT()
    i.type = INPUT_MOUSE
    i.u.mi = MOUSEINPUT(nx, ny, 0, MOUSEEVENTF_MOVE_ABS, 0, None)
    _send(i)


def keep_awake() -> None:
    k32.SetThreadExecutionState(
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    )


# ───────────────────────── 功能开关 ─────────────────────────
def enable_features() -> None:
    """备份并打开视觉类功能（音效保持关闭，半夜不吵人）。"""
    if os.path.exists(CONFIG_BAK):
        print("已存在配置备份 config.json.bak_soak（不覆盖）")
    else:
        shutil.copyfile(CONFIG, CONFIG_BAK)
        print("已备份原配置 -> config.json.bak_soak")
    with open(CONFIG, encoding="utf-8") as f:
        cfg = json.load(f)
    cfg["trail"]["trail_enabled"] = True       # ribbon 拖尾
    cfg["trail"]["ripple_enabled"] = True      # 点击波纹（left=spray）
    cfg["trail"]["hover_enabled"] = True       # 悬浮标 solid
    cfg["key_display"]["enabled"] = True       # 击键回显
    # fade_delay 10→2：默认 10s 会让气泡堆成一面黄墙盖住画面（预演实测）。
    cfg["key_display"]["fade_delay"] = 2
    cfg["virtual_keyboard"]["enabled"] = True  # 虚拟键盘键帽高亮
    with open(CONFIG, "w", encoding="utf-8", newline="") as f:
        json.dump(cfg, f, ensure_ascii=False, indent=1)
    print("功能开关：拖尾/波纹/悬浮标/击键回显/虚拟键盘 = ON；音效 = OFF")


# ───────────────────────── 记事本沙箱 ─────────────────────────
def start_notepad() -> int:
    """起记事本并返回它的主窗口 hwnd。

    ★ 不能用 GetForegroundWindow 抓：CurSeen 面板会抢前台（实测抓到的是面板
    hwnd=0x1730856，打字全打进了被测程序）。按 pid 枚举窗口才可靠。
    """
    proc = subprocess.Popen(["notepad.exe"])
    time.sleep(2.5)
    hwnd = _find_window_by_pid(proc.pid)
    if not hwnd:
        # Win11 的 notepad 可能经 app alias 中转（pid 对不上），退回前台法
        hwnd = u32.GetForegroundWindow()
        print("!! 按 pid 没找到记事本窗口，退回前台法（务必核对截图）", flush=True)
    SW_MAXIMIZE = 3
    u32.ShowWindow(hwnd, SW_MAXIMIZE)
    u32.SetForegroundWindow(hwnd)
    time.sleep(0.5)
    return hwnd


def _find_window_by_pid(pid: int) -> int:
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _l):
        p = wintypes.DWORD()
        u32.GetWindowThreadProcessId(h, ctypes.byref(p))
        if p.value == pid and u32.IsWindowVisible(h) and u32.GetWindowTextLengthW(h) > 0:
            found.append(h)
            return False
        return True

    u32.EnumWindows(cb, 0)
    return found[0] if found else 0


def notepad_rect(hwnd: int):
    """窗口矩形 (l, t, r, b)；窗口无效返回 None。点击必须落在里面 ——
    预演实测：点歪到桌面会拖乱图标、点歪到 CurSeen 面板会乱翻页面。"""
    class RECTS(ctypes.Structure):
        _fields_ = [
            ("l", wintypes.LONG), ("t", wintypes.LONG),
            ("r", wintypes.LONG), ("b", wintypes.LONG),
        ]
    if not hwnd or not u32.IsWindow(hwnd):
        return None
    rc = RECTS()
    if not u32.GetWindowRect(hwnd, ctypes.byref(rc)):
        return None
    if rc.r - rc.l < 200 or rc.b - rc.t < 150:
        return None
    return (rc.l, rc.t, rc.r, rc.b)


# ───────────────────────── 活动剧本 ─────────────────────────
def activity_round(hwnd: int, n: int, shots: list | None = None) -> None:
    """一轮 ~40s 的真实输入活动：拖尾 + 打字 + 点击。

    `shots` 非空时在第 1、2 个位置各存一张截图（仅预演用）：拖尾要在画圈
    **途中**拍（fade 1s，等活动轮结束早就消散了），波纹要在点击后立刻拍。
    """
    sw, sh = u32.GetSystemMetrics(0), u32.GetSystemMetrics(1)
    cx, cy = sw // 2, sh // 2

    # 记事本矩形：所有点击都限制在里面（找不到就只做移动类活动）。
    rect = notepad_rect(hwnd)
    if rect:
        bx0, by0, bx1, by1 = rect
        bx0, by0 = bx0 + 60, by0 + 120   # 躲开标题栏和菜单栏
        bx1, by1 = bx1 - 60, by1 - 80
    else:
        bx0 = by0 = bx1 = by1 = 0

    # 1) 画 Lissajous 曲线 12s（拖尾素材）—— SendInput 绝对移动（走 LL 钩子）
    t0 = time.time()
    shot_a_done = False
    while time.time() - t0 < 12:
        t = time.time() - t0
        x = cx + int(math.sin(t * 1.7) * sw * 0.33)
        y = cy + int(math.sin(t * 2.3 + 1.2) * sh * 0.30)
        move_abs(x, y, sw, sh)
        if shots and not shot_a_done and t > 10:
            shot_a_done = True
            screenshot(shots[0])
        time.sleep(0.016)

    # 2) 点击记事本拿焦点（顺带出波纹），打字 8s（击键回显 + 虚拟键盘）
    if rect:
        click((bx0 + bx1) // 2, (by0 + by1) // 2)
        u32.SetForegroundWindow(hwnd)
        # ★ 切 en-US 键盘布局绕开拼音 IME：否则合成字母被 IME 组合成中文候选
        #   （预演实测 "curseen" 上屏成"粗染色"），候选窗还会盖住画面。
        WM_INPUTLANGCHANGEREQUEST = 0x0050
        u32.PostMessageW(hwnd, WM_INPUTLANGCHANGEREQUEST, 0, 0x0409)
        # 再给 IME 默认窗口直发"字母模式"（中文拼音 IME 的 NATIVE 态会把合成
        # 字母上屏成中文候选，预演实测）。三道保险：转换模式 + Ctrl+Space + Shift。
        # ★ ImmGetDefaultIMEWnd 在 **imm32.dll**（user32 里没有，之前崩过一次）。
        try:
            imm32 = ctypes.WinDLL("imm32")
            ime_wnd = imm32.ImmGetDefaultIMEWnd(hwnd)
            WM_IME_CONTROL, IMC_SETCONVERSIONMODE = 0x0283, 0x002
            u32.SendMessageW(ime_wnd, WM_IME_CONTROL, IMC_SETCONVERSIONMODE, 0)
        except Exception as e:  # IME 切换失败不影响测试本身
            print(f"(IME 切换跳过: {e})", flush=True)
        tap(0x11)  # Ctrl
        tap(VK_SPACE)
        tap(VK_SHIFT)
        time.sleep(0.3)
        t0 = time.time()
        words = "curseen soak test "
        while time.time() - t0 < 8:
            for ch in words:
                type_char(ch)
                if time.time() - t0 >= 8:
                    break

    # 3) 随机点击 10 次（波纹 spray）；第 2 次点击后立刻截一张（波纹 300ms 级）
    if rect:
        for k in range(10):
            click(random.randint(bx0, bx1), random.randint(by0, by1))
            if shots and k == 1:
                time.sleep(0.15)
                screenshot(shots[1])
            time.sleep(0.5)

    # 4) 回车换行 + 退格清理一点，制造键帽差异
    if rect:
        tap(VK_RETURN)
        for _ in range(5):
            tap(VK_BACK)

    print(f"  活动第 {n} 轮完成（{datetime.now():%H:%M:%S}）", flush=True)


def screenshot(path: str) -> None:
    from PIL import ImageGrab

    ImageGrab.grab().save(path)
    print(f"截图：{path}", flush=True)


# ───────────────────────── 提权 ─────────────────────────
# ★★★ curseen.exe 是高完整性进程（提权运行）：中完整性脚本合成输入会被
#   UIPI **静默丢弃**（实测：SendInput 返回成功但光标不动、键帽不亮）。
#   本机 UAC=从不通知（ConsentPromptBehaviorAdmin=0），runas 静默放行 ⇒
#   脚本检测未提权就 runas 重启自己（--elevated），输出写日志文件。


class SHELLEXECUTEINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.DWORD),
        ("fMask", wintypes.ULONG),
        ("hwnd", wintypes.HWND),
        ("lpVerb", wintypes.LPCWSTR),
        ("lpFile", wintypes.LPCWSTR),
        ("lpParameters", wintypes.LPCWSTR),
        ("lpDirectory", wintypes.LPCWSTR),
        ("nShow", ctypes.c_int),
        ("hInstApp", wintypes.HINSTANCE),
        ("lpIDList", ctypes.c_void_p),
        ("lpClass", wintypes.LPCWSTR),
        ("hkeyClass", wintypes.HKEY),
        ("dwHotKey", wintypes.DWORD),
        ("hIconOrMonitor", wintypes.HANDLE),
        ("hProcess", wintypes.HANDLE),
    ]


SEE_MASK_NOCLOSEPROCESS = 0x00000040
INFINITE = 0xFFFFFFFF


def relaunch_elevated(log_path: str) -> int:
    """runas 重启自己并**等待**退出（父进程在 job 里挂着，子进程才不被回收）。
    返回子进程退出码。"""
    args = subprocess.list2cmdline(sys.argv[1:] + ["--elevated", log_path])
    sei = SHELLEXECUTEINFO()
    sei.cbSize = ctypes.sizeof(sei)
    sei.fMask = SEE_MASK_NOCLOSEPROCESS
    sei.lpVerb = "runas"
    sei.lpFile = sys.executable
    sei.lpParameters = f'"{os.path.abspath(sys.argv[0])}" {args}'
    sei.nShow = 1
    if not ctypes.windll.shell32.ShellExecuteExW(ctypes.byref(sei)):
        print(f"提权重启失败（{ctypes.get_last_error()}）——无法驱动提权中的 curseen.exe")
        return 2
    k32.WaitForSingleObject(sei.hProcess, INFINITE)
    code = wintypes.DWORD()
    k32.GetExitCodeProcess(sei.hProcess, ctypes.byref(code))
    k32.CloseHandle(sei.hProcess)
    return code.value


# ───────────────────────── 主流程 ─────────────────────────
def main() -> int:
    dry = "--dry" in sys.argv
    log_path = sys.argv[sys.argv.index("--elevated") + 1] if "--elevated" in sys.argv else ""
    stamp = datetime.now().strftime("%Y%m%d_%H%M")
    if "--elevated" not in sys.argv and not ctypes.windll.shell32.IsUserAnAdmin():
        print("当前进程非管理员：runas 重启（UAC=从不通知，静默放行）…", flush=True)
        return relaunch_elevated(log_path or os.path.join(ROOT, "temp", f"soak_drive_{stamp}.log"))
    if log_path:
        # 提权子进程：输出重定向到日志（父进程事后读）。
        logf = open(log_path, "w", encoding="utf-8", buffering=1)
        sys.stdout = sys.stderr = logf
    print(f"== 长测联动驱动启动 {stamp}（dry={dry}，提权={bool(ctypes.windll.shell32.IsUserAnAdmin())}）==",
          flush=True)

    # 单实例检查：curseen 已在跑的话单实例闸门会把新进程弹掉。
    r = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq curseen.exe"],
        capture_output=True,
    )
    if b"curseen.exe" in (r.stdout or b"").lower():
        print("!! curseen.exe 已在运行 —— 特效可能由旧实例承载，配置改动未必生效")
        print("   （继续跑，但预演截图要重点核对特效）")
    else:
        print("curseen.exe 未运行，将启动部署版")

    enable_features()

    # 旧实例必须先退：配置改动（fade_delay 等）要重启 exe 才生效。
    subprocess.run(["taskkill", "/F", "/IM", "curseen.exe"], capture_output=True)
    time.sleep(2)
    subprocess.Popen([os.path.join(BUILD, "curseen.exe")])
    print("curseen.exe 已启动，等 6s 初始化…", flush=True)
    time.sleep(6)

    hwnd = start_notepad()
    print(f"记事本沙箱 hwnd=0x{hwnd:X}", flush=True)

    if dry:
        base = os.path.join(ROOT, "temp", f"soak_dry_{stamp}")
        activity_round(hwnd, 1, shots=[base + "_a_trail.png", base + "_b_ripple.png"])
        subprocess.run(["taskkill", "/F", "/IM", "notepad.exe"],
                       capture_output=True)
        print("== 预演结束（curseen.exe 保持运行）==", flush=True)
        return 0

    # ── 全量：探针（录制判据主体）与驱动并行 ──
    env = dict(os.environ)
    env["SOAK_LINK_SECS"] = "7200"
    env["SOAK_LINK_AV_MARKER"] = "1"
    probe = subprocess.Popen(
        [sys.executable, "build/probe.py", "run", "soak_link"],
        cwd=MAIN, env=env,
        stdout=open(os.path.join(ROOT, "temp", f"soak2h_{stamp}.log"), "w",
                    encoding="utf-8", errors="replace"),
        stderr=subprocess.STDOUT,
    )
    print(f"探针已启动 pid={probe.pid}（7200s，音画标记开）", flush=True)

    n = 0
    t_start = time.time()
    while probe.poll() is None:
        # ★ 活动轮整轮兜底：单轮里任何 Win32 调用失败都不许打断 2h 测试
        #   （驱动一死就没人防熄屏了 —— 第一轮实测 ImmGetDefaultIMEWnd
        #   放错 DLL 直接带崩全场）。防熄屏放 finally，异常时也必须执行。
        try:
            n += 1
            activity_round(hwnd, n)
        except Exception as e:
            print(f"!! 第 {n} 轮活动异常（继续）: {e!r}", flush=True)
        finally:
            keep_awake()
            time.sleep(2)

    rc = probe.returncode
    print(f"探针退出 rc={rc}，驱动总时长 {(time.time() - t_start) / 60:.1f} 分钟",
          flush=True)

    # ── 收尾：关沙箱与被测 exe，改名产物 ──
    subprocess.run(["taskkill", "/F", "/IM", "notepad.exe"], capture_output=True)
    subprocess.run(["taskkill", "/F", "/IM", "curseen.exe"], capture_output=True)

    os.makedirs(OUTDIR, exist_ok=True)
    final = os.path.join(OUTDIR, f"联动长测2h_{stamp}.mp4")
    src = os.path.join(PROD_SRC, "soak_link_7200s.mp4")
    if os.path.exists(src):
        shutil.move(src, final)
        size_gb = os.path.getsize(final) / 1e9
        print(f"产物已保留：{final}（{size_gb:.2f} GB）", flush=True)
    else:
        print(f"!! 没找到产物 {src} —— 探针可能失败，看日志", flush=True)

    summary = os.path.join(OUTDIR, f"长测摘要_{stamp}.txt")
    with open(summary, "w", encoding="utf-8") as f:
        f.write(f"长测时间：{stamp}\n探针退出码：{rc}\n产物：{final}\n")
        f.write("功能开关：拖尾/波纹/悬浮标/击键回显/虚拟键盘 ON；音效 OFF\n")
        f.write("配置备份：build/config.json.bak_soak\n")
        f.write("探针日志：temp/soak2h_" + stamp + ".log\n")
    print(f"摘要：{summary}", flush=True)
    return 0 if rc == 0 and os.path.exists(final) else 1


if __name__ == "__main__":
    sys.exit(main())
