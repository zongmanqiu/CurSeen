#!/usr/bin/env python3
"""音画同步标记驱动（配合 `tests/probes/soak_link.rs` 的 `SOAK_LINK_AV_MARKER=1`）。

由探针在**起录成功后**拉起，做两件事（同一时钟起步，供录完后做音画同步分析）：

1. 播参考音：440Hz 连续 + 每 5s 一个 1760Hz 标记 0.3s（8kHz 单声道 16-bit WAV，
   5s 块平铺生成一次、缓存复用）。录屏走系统 loopback，声音原样进产物音轨。
2. 屏幕白闪：一个置顶小窗每 5s 白 0.5s / 黑 4.5s，与音频标记同钟对齐。
   录屏把它原样收进视频轨 ⇒「视频里的白闪时刻」对「音频里的标记时刻」
   之差随时间的漂移曲线，就是音画同步漂移。

实现注记：白闪窗用 **ctypes 直建 Win32 窗口**（不依赖 tkinter —— 托管 Python
3.13 不带它；也不搞"换解释器重启自身"—— 探针按 pid kill 一次就干净）。

时长：`MARKER_SECS` 环境变量（默认 300）。到点自退；探针收尾时也会兜底 kill。

跑法（一般不手动跑）：探针内部 `python build/av_marker_driver.py`。
"""

import ctypes
import math
import os
import struct
import sys
import time
import wave
import winsound

HERE = os.path.dirname(os.path.abspath(__file__))
# main/build/av_marker_driver.py → 仓库根 temp/（探针产物与音文件都按 main/ 的
# `../temp` 口径走，即 CurSeen/temp；一次性大文件放这里，不进 main/）。
TEMP = os.path.normpath(os.path.join(HERE, "..", "..", "temp"))

# ── 参考音参数（与 audio_e2e 的标记检测约定一致）──
SAMPLE_RATE = 8000          # 8kHz 足够放下 440/1760Hz，2 小时只有 ~115MB
BASE_HZ = 440.0             # 连续参考音（查断流/电流声）
MARK_HZ = 1760.0            # 标记音（查标记间隔精确性 + 音画同步）
MARK_LEN_S = 0.3            # 标记时长
BLOCK_S = 5                 # 标记周期（块长，平铺单位）
BASE_AMP = 0.28
MARK_AMP = 0.42

# ── 白闪窗参数 ──
FLASH_S = 0.5               # 每个标记对齐一次白闪

# 点击模式（`AV_CLICK=1`）：不在整段里铺一条持续播放会话，而是**每次白闪瞬间播
# 一声 0.3s 短响、播完会话即结束** —— 复现"平时系统完全没声音、用户点一下才响一声"
# 的真实场景（用户报"声音比画面慢"就是在这个场景里听出来的）。
# 持续会话下 loopback 一直产包，静默期的音画行为被掩盖，测不出偏移。
CLICK_MODE = bool(os.environ.get("AV_CLICK"))
WIN_SIZE = 480
WIN_MARGIN = 24


def build_tone(total_s: int) -> str:
    """生成（或复用）total_s 秒参考音 WAV，返回路径。"""
    total_s = ((total_s + BLOCK_S - 1) // BLOCK_S) * BLOCK_S
    # `AV_BASE_AMP=0` ⇒ **稀疏模式**：整段只有每 5s 一个标记，其余全静音。
    # 用户报的"声音比画面慢"是在**静默里的一声短响**（点系统音量的"叮"）上听出来的；
    # 连续参考音下 loopback 一直产包，静音填充和两路对齐的毛病全被掩盖，
    # 实测出来的偏移是好绿但没意义（2026-10-09 实测：连续音 +33ms，稀疏模式才露馅）。
    base_amp = float(os.environ.get("AV_BASE_AMP", str(BASE_AMP)))
    tag = "sparse" if base_amp == 0.0 else "cont"
    path = os.path.join(TEMP, f"av_tone_{total_s}s_{tag}.wav")
    if os.path.exists(path) and os.path.getsize(path) > 44:
        return path
    os.makedirs(TEMP, exist_ok=True)

    n = SAMPLE_RATE * BLOCK_S
    mark_n = int(SAMPLE_RATE * MARK_LEN_S)
    block = bytearray(n * 2)
    for i in range(n):
        v = base_amp * math.sin(2.0 * math.pi * BASE_HZ * i / SAMPLE_RATE)
        if i < mark_n:
            # 标记与基准叠放：Goertzel 单频检测互不干扰
            v += MARK_AMP * math.sin(2.0 * math.pi * MARK_HZ * i / SAMPLE_RATE)
        struct.pack_into("<h", block, i * 2, int(max(-1.0, min(1.0, v)) * 32767))
    reps = total_s // BLOCK_S
    # ★ 先写 .tmp 再原子改名：长音生成要几十秒，若直接写目标名，另一个进程
    #   （或下次启动）看到"已存在且 >44B"就会拿半成品去播。
    tmp = path + ".tmp"
    with wave.open(tmp, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(bytes(block) * reps)
    os.replace(tmp, path)
    return path


def build_click() -> str:
    """一声 0.3s 短响（点击模式用）：播完即结束播放会话，之后系统彻底无声。"""
    path = os.path.join(TEMP, "av_click_03s.wav")
    if os.path.exists(path) and os.path.getsize(path) > 44:
        return path
    os.makedirs(TEMP, exist_ok=True)
    n = int(SAMPLE_RATE * MARK_LEN_S)
    buf = bytearray(n * 2)
    for i in range(n):
        v = MARK_AMP * math.sin(2.0 * math.pi * MARK_HZ * i / SAMPLE_RATE)
        struct.pack_into("<h", buf, i * 2, int(max(-1.0, min(1.0, v)) * 32767))
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(bytes(buf))
    return path


# ────────────────────────── ctypes 白闪窗 ──────────────────────────

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
kernel32 = ctypes.windll.kernel32

# ★ 不设 argtypes 时 ctypes 默认按 c_int 传参：lParam（c_ssize_t）大值会
#   "int too long to convert"，窗口消息处理全炸（实测）。一次性声明全。
WNDPROC_TYPE = ctypes.WINFUNCTYPE(
    ctypes.c_ssize_t, ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t
)
user32.DefWindowProcW.argtypes = [
    ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t
]
user32.DefWindowProcW.restype = ctypes.c_ssize_t
user32.CreateWindowExW.argtypes = [
    ctypes.c_uint, ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint,
    ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
]
user32.CreateWindowExW.restype = ctypes.c_void_p
user32.FindWindowW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p]
user32.FindWindowW.restype = ctypes.c_void_p
user32.BeginPaint.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.BeginPaint.restype = ctypes.c_void_p
user32.EndPaint.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.FillRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
gdi32.CreateSolidBrush.argtypes = [ctypes.c_uint]
gdi32.CreateSolidBrush.restype = ctypes.c_void_p
gdi32.DeleteObject.argtypes = [ctypes.c_void_p]
user32.GetClientRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.InvalidateRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
user32.UpdateWindow.argtypes = [ctypes.c_void_p]
user32.SetTimer.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint, ctypes.c_void_p]
user32.SetWindowPos.argtypes = [
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int,
    ctypes.c_int, ctypes.c_int, ctypes.c_uint,
]
user32.ShowWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
user32.GetMessageW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint]
user32.GetSystemMetrics.argtypes = [ctypes.c_int]
user32.GetSystemMetrics.restype = ctypes.c_int
kernel32.GetModuleHandleW.argtypes = [ctypes.c_wchar_p]
kernel32.GetModuleHandleW.restype = ctypes.c_void_p

WNDPROC = ctypes.WINFUNCTYPE(
    ctypes.c_ssize_t, ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t
)


class WNDCLASSW(ctypes.Structure):
    _fields_ = [
        ("style", ctypes.c_uint),
        ("lpfnWndProc", WNDPROC),
        ("cbClsExtra", ctypes.c_int),
        ("cbWndExtra", ctypes.c_int),
        ("hInstance", ctypes.c_void_p),
        ("hIcon", ctypes.c_void_p),
        ("hCursor", ctypes.c_void_p),
        ("hbrBackground", ctypes.c_void_p),
        ("lpszMenuName", ctypes.c_wchar_p),
        ("lpszClassName", ctypes.c_wchar_p),
    ]


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


class POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


class MSG(ctypes.Structure):
    _fields_ = [
        ("hwnd", ctypes.c_void_p),
        ("message", ctypes.c_uint),
        ("wParam", ctypes.c_size_t),
        ("lParam", ctypes.c_ssize_t),
        ("time", ctypes.c_uint),
        ("pt", POINT),
    ]


WM_PAINT = 0x000F
WM_DESTROY = 0x0002
WM_TIMER = 0x0113
SM_CXSCREEN = 0
SW_SHOWNOACTIVATE = 4
HWND_TOPMOST = -1
SWP_NOMOVE = 0x2
SWP_NOSIZE = 0x1

state = {"hwnd": None, "white": False, "t0": 0.0, "secs": 0}


def flash_on(now: float) -> bool:
    """白闪窗当前该不该是白的：每个 5s 块的前 0.5s。"""
    return (now % BLOCK_S) < FLASH_S


def wnd_proc(hwnd, msg, wParam, lParam):
    if msg == WM_PAINT:
        # PAINTSTRUCT 首成员是 hdc，直接按内存布局取。
        buf = ctypes.create_string_buffer(64)
        hdc = user32.BeginPaint(hwnd, buf)
        brush = gdi32.CreateSolidBrush(0x00FFFFFF if state["white"] else 0x00000000)
        rc = RECT()
        user32.GetClientRect(hwnd, ctypes.byref(rc))
        user32.FillRect(hdc, ctypes.byref(rc), brush)
        gdi32.DeleteObject(brush)
        user32.EndPaint(hwnd, buf)
        return 0
    if msg == WM_TIMER:
        now = time.monotonic() - state["t0"]
        if now > state["secs"] + 5:
            user32.PostQuitMessage(0)
            return 0
        want = flash_on(now)
        if want != state["white"]:
            state["white"] = want
            user32.InvalidateRect(hwnd, None, False)
            user32.UpdateWindow(hwnd)
            # 点击模式：白闪刚亮起的同一刻播一声短响（声画同源、同钟起步）。
            if want and CLICK_MODE:
                winsound.PlaySound(
                    state["click"], winsound.SND_FILENAME | winsound.SND_ASYNC
                )
                print(f"AV_CLICK at {now:.3f}s", flush=True)
        return 0
    if msg == WM_DESTROY:
        user32.PostQuitMessage(0)
        return 0
    return user32.DefWindowProcW(hwnd, msg, wParam, lParam)


def run_flash_window(secs: int) -> None:
    state["secs"] = secs
    hinst = kernel32.GetModuleHandleW(None)
    cls = "CurSeenAvFlash"
    wc = WNDCLASSW()
    wc.lpfnWndProc = WNDPROC(wnd_proc)
    wc.hInstance = hinst
    wc.lpszClassName = cls
    if not user32.RegisterClassW(ctypes.byref(wc)):
        sys.exit("FAIL RegisterClassW")
    sw = user32.GetSystemMetrics(SM_CXSCREEN)
    x = sw - WIN_SIZE - WIN_MARGIN
    y = WIN_MARGIN
    hwnd = user32.CreateWindowExW(
        0x08000000,          # WS_EX_NOACTIVATE：不抢焦点
        cls,
        "avflash",
        0x90000000,          # WS_POPUP | WS_VISIBLE
        x, y, WIN_SIZE, WIN_SIZE,
        None, None, hinst, None,
    )
    if not hwnd:
        sys.exit("FAIL CreateWindowExW")
    state["hwnd"] = hwnd
    # 声音先起（异步、整段一次播完，无循环接缝），窗口时钟随后对表：
    # 两者都是实时钟，起步偏移恒定 ⇒ 漂移分析只看斜率，不看绝对对齐。
    if CLICK_MODE:
        state["click"] = build_click()
        print(f"AV_DRIVER_CLICK {state['click']}（点击模式：仅白闪瞬间有播放会话）", flush=True)
    else:
        tone = build_tone(secs)
        print(f"AV_DRIVER_TONE {tone}", flush=True)
        winsound.PlaySound(tone, winsound.SND_FILENAME | winsound.SND_ASYNC)
    state["t0"] = time.monotonic()
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    user32.ShowWindow(hwnd, SW_SHOWNOACTIVATE)
    user32.InvalidateRect(hwnd, None, False)
    user32.SetTimer(hwnd, 1, 50, None)   # 50ms 轮询：白闪沿足够陡（远小于 33ms 帧间隔的 2 倍）

    msg = MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))
    winsound.PlaySound(None, 0)  # 停声


def main() -> int:
    secs = int(os.environ.get("MARKER_SECS", "300"))
    print(f"AV_DRIVER_READY {time.time():.3f} secs={secs}", flush=True)
    run_flash_window(secs)
    print(f"AV_DRIVER_DONE {time.time():.3f}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
