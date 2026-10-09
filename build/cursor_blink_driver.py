# -*- coding: utf-8 -*-
"""I-beam 光标闪烁复现驱动（2026-10-09）—— 配合 tests/probes/cursor_flicker.rs。

复现用户场景：「TIM 里输入数字，I-beam 光标在录出来的视频里闪烁厉害」。要素：

1. **I-beam 形态**：窗口类光标设 IDC_IBEAM，鼠标 SetCursorPos 停在窗口正中
   （类光标由 DefWindowProc 在 WM_SETCURSOR 自动套用，与文本框行为一致）。
2. **静屏 + 稀疏更新**：背景纯色不动，只有一小条"插入符"以 2Hz 闪烁
   （InvalidateRect 局部重绘）——这正是打字时的桌面更新节奏：大部分 tick
   AcquireNextFrame 超时、偶有 fresh 帧。

输出协议：`CURSOR_DRV_READY <中心绝对x> <中心绝对y>`（探针等这行再起录）。
时长/更新频率由环境变量 `MARKER_SECS` / `BLINK_MS` 控制（默认 8s / 500ms）。
"""

import ctypes
import os
import time

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
kernel32 = ctypes.windll.kernel32

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
user32.BeginPaint.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.BeginPaint.restype = ctypes.c_void_p
user32.EndPaint.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.FillRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
gdi32.CreateSolidBrush.argtypes = [ctypes.c_uint]
gdi32.CreateSolidBrush.restype = ctypes.c_void_p
gdi32.DeleteObject.argtypes = [ctypes.c_void_p]
user32.LoadCursorW.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
user32.LoadCursorW.restype = ctypes.c_void_p
user32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
user32.SetForegroundWindow.argtypes = [ctypes.c_void_p]
user32.SetTimer.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint, ctypes.c_void_p]
user32.KillTimer.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
user32.InvalidateRect.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
user32.UpdateWindow.argtypes = [ctypes.c_void_p]
user32.ShowWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
user32.GetMessageW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint]
user32.PeekMessageW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint]
user32.TranslateMessage.argtypes = [ctypes.c_void_p]
user32.DispatchMessageW.argtypes = [ctypes.c_void_p]
user32.DestroyWindow.argtypes = [ctypes.c_void_p]
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


class PAINTSTRUCT(ctypes.Structure):
    _fields_ = [
        ("hdc", ctypes.c_void_p),
        ("fErase", ctypes.c_int),
        ("rcPaint", RECT),
        ("fRestore", ctypes.c_int),
        ("fIncUpdate", ctypes.c_int),
        ("rgbReserved", ctypes.c_ubyte * 32),
    ]


IDC_IBEAM = 32513  # MAKEINTRESOURCE：系统光标是整数资源 ID，传字符串得 NULL
WM_TIMER = 0x0113
WM_PAINT = 0x000F
WM_DESTROY = 0x0002
WM_SETCURSOR = 0x0020
HTCLIENT = 1
SW_SHOW = 5

WIN_X, WIN_Y, WIN_W, WIN_H = 100, 100, 640, 480
BG_BRUSH = 0x00F0F0F0  # COLORREF = 0x00BBGGRR → 浅灰 (240,240,240)
CARET_BRUSH = 0x00000000  # 黑
CARET_W = 4
CARET_H = 32

state = {"caret_on": False, "hwnd": None, "hcursor": None}


def wnd_proc(hwnd, msg, wparam, lparam):
    if msg == WM_SETCURSOR and (lparam & 0xFFFF) == HTCLIENT:
        # 客户区上显式套 I-beam：SetCursorPos 不产生 WM_MOUSEMOVE，
        # 光标样式可能一直停留在旧窗（箭头），探针的句柄校验会作废。
        user32.SetCursor(state["hcursor"])
        return 1


def wnd_proc(hwnd, msg, wparam, lparam):
    if msg == WM_TIMER and wparam == 1:
        state["caret_on"] = not state["caret_on"]
        user32.InvalidateRect(hwnd, None, False)
        user32.UpdateWindow(hwnd)
        return 0
    if msg == WM_TIMER and wparam == 2:
        # 动画：色块横移（30fps 级桌面更新，制造「TIM 有动画/重绘」的 fresh 帧节奏）
        state["anim_x"] = (state.get("anim_x", 20) + 8) % (WIN_W - 60)
        user32.InvalidateRect(hwnd, None, False)
        user32.UpdateWindow(hwnd)
        return 0
    if msg == WM_PAINT:
        ps = PAINTSTRUCT()
        hdc = user32.BeginPaint(hwnd, ctypes.byref(ps))
        # 背景：整块浅灰（CreateWindow 的类背景在 fErase 时已刷，这里保险再刷一次）
        bg = gdi32.CreateSolidBrush(BG_BRUSH)
        client = RECT(0, 0, WIN_W, WIN_H)
        user32.FillRect(hdc, ctypes.byref(client), bg)
        gdi32.DeleteObject(bg)
        # 插入符：光标右侧一小条，2Hz 亮灭
        if state.get("anim_x") is not None:
            bar = gdi32.CreateSolidBrush(0x0000C0C0)  # 黄色块
            ax = state["anim_x"]
            r = RECT(ax, WIN_H - 80, ax + 40, WIN_H - 40)
            user32.FillRect(hdc, ctypes.byref(r), bar)
            gdi32.DeleteObject(bar)
        if state["caret_on"]:
            bar = gdi32.CreateSolidBrush(CARET_BRUSH)
            r = RECT(WIN_W // 2 + 40, WIN_H // 2 - CARET_H // 2,
                     WIN_W // 2 + 40 + CARET_W, WIN_H // 2 + CARET_H // 2)
            user32.FillRect(hdc, ctypes.byref(r), bar)
            gdi32.DeleteObject(bar)
        user32.EndPaint(hwnd, ctypes.byref(ps))
        return 0
    if msg == WM_DESTROY:
        return 0
    return user32.DefWindowProcW(hwnd, msg, wparam, lparam)


def main():
    secs = float(os.environ.get("MARKER_SECS", "8"))
    blink_ms = int(os.environ.get("BLINK_MS", "500"))

    hinst = kernel32.GetModuleHandleW(None)
    proc = WNDPROC(wnd_proc)
    wc = WNDCLASSW()
    wc.lpfnWndProc = proc
    wc.hInstance = hinst
    wc.hCursor = user32.LoadCursorW(None, IDC_IBEAM)
    wc.hbrBackground = gdi32.CreateSolidBrush(BG_BRUSH)
    wc.lpszClassName = "CurSeenCursorFlickerDrv"
    if not user32.RegisterClassW(ctypes.byref(wc)):
        print("CURSOR_DRV_ERR RegisterClassW failed", flush=True)
        return 1
    hwnd = user32.CreateWindowExW(
        0x8, wc.lpszClassName, "I-beam 闪烁复现", 0x00CF0000 | 0x00000000,
        WIN_X, WIN_Y, WIN_W, WIN_H, None, None, hinst, None,
    )  # WS_EX_TOPMOST：探针抓的是屏幕，被别的窗盖住判据就废了
    if not hwnd:
        print("CURSOR_DRV_ERR CreateWindowExW failed", flush=True)
        return 1
    state["hwnd"] = hwnd
    user32.ShowWindow(hwnd, SW_SHOW)
    user32.UpdateWindow(hwnd)
    user32.SetForegroundWindow(hwnd)

    # 鼠标停在窗口正中：类光标（I-beam）在这里生效
    cx, cy = WIN_X + WIN_W // 2, WIN_Y + WIN_H // 2
    state["hcursor"] = user32.LoadCursorW(None, IDC_IBEAM)
    user32.SetCursorPos(cx, cy)
    time.sleep(0.2)
    user32.SetCursorPos(cx, cy)
    user32.SetCursor(state["hcursor"])  # 兜底：无移动事件也把形态立起来

    user32.SetTimer(hwnd, 1, blink_ms, None)
    anim_ms = os.environ.get("ANIM_MS", "")
    if anim_ms:
        user32.SetTimer(hwnd, 2, int(anim_ms), None)
    print(f"CURSOR_DRV_READY {cx} {cy}", flush=True)

    t0 = time.monotonic()
    PM_REMOVE = 1
    buf = (ctypes.c_ubyte * 48)()  # MSG 结构占位（64 位下 48 字节）
    while time.monotonic() - t0 < secs:
        # PeekMessage 轮询泵消息：WM_TIMER → 翻转插入符 → 局部重绘（产生 fresh 帧）。
        while user32.PeekMessageW(buf, None, 0, 0, PM_REMOVE):
            user32.TranslateMessage(buf)
            user32.DispatchMessageW(buf)
        time.sleep(0.01)
    user32.KillTimer(hwnd, 1)
    user32.DestroyWindow(hwnd)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
