-- CursorFX · clock / 复刻自 cursorfx-main/src/cursors/clock.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：一个真实走动的模拟表盘，挂在指针右下角；12 个刻度、时/分/秒三针，表盘
-- 下方附一行正立数字时间。指针用 line 绘制，不需要画布旋转。
--
-- 时间源：`local_ms`（宿主每帧注入的本地 Unix 毫秒）→ 默认就是系统本地时间；
-- `offset`（小时）只用于手动校正。`wall_ms` 是系统运行毫秒，不能当时钟用。
--
-- 性能：2 个圆 + 1 环 + 12 条刻度 + 3 条指针 + 1 个中心点 + 1 行文字 + 指针双色点
-- ≈ 23 个图元，开销可忽略。

name = "clock"
effect_name = "clock"
kind = "cursor"
description = "CursorFX clock：跟随指针的模拟表盘（含数字时间）。"
hide_cursor = true

local TAU = math.pi * 2.0

function schema()
  return {
    { key = "size",    label = "表盘直径", type = "int",   min = 28, max = 100, step = 1,    default = 44 },
    { key = "color",   label = "指针/刻度", type = "color", default = "#FFFFFF" },
    { key = "face",    label = "表盘底色", type = "color", alpha = true, default = "#141828D9" },
    { key = "accent",  label = "秒针颜色", type = "color", default = "#FF5252" },
    { key = "offset",  label = "时间校正(小时)", type = "float", min = -12, max = 12, step = 0.25, default = 0 },
    { key = "digital", label = "数字时间", type = "bool", default = true },
    { key = "dot_color",   label = "指针点颜色", type = "color", default = "#FFFFFF" },
    { key = "dot_outline", label = "指针点描边", type = "color", default = "#000000" },
    { key = "dot_size",    label = "指针点大小", type = "int", min = 2, max = 8, step = 1, default = 3 },
  }
end

local px, py = 0, 0
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  px, py = 0, 0
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    px, py = mx, my
    started = true
  end
  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k = math.min(1.0, 0.25 * f)
  px = px + (mx - px) * k
  py = py + (my - py) * k
end

local function hand(ctx, cx, cy, value, total, len, width, cr, cg, cb, ca)
  local a = (value / total) * TAU - math.pi / 2.0
  local dx, dy = math.cos(a), math.sin(a)
  ctx:line(cx - dx * 3.0, cy - dy * 3.0, cx + dx * len, cy + dy * len, width, cr, cg, cb, ca)
end

function draw(ctx, params)
  if not started then return end

  local R = math.max((params.size or 44) * 0.5, 10.0)
  local cx = px + R + 8.0
  local cy = py + R + 8.0

  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFFFFF")
  local fr, fg, fb, fa = hex_to_rgb(params.face or "#141828D9")
  local ar, ag, ab, aa = hex_to_rgb(params.accent or "#FF5252")

  -- 时间：宿主注入的本地 Unix 毫秒 + 手动小时校正。小时/分钟/秒都保留小数，表针才连续。
  local base_ms = local_ms or wall_ms or 0
  local seconds = base_ms / 1000.0 + (params.offset or 0) * 3600.0
  local sec = seconds % 60.0
  local total_min = seconds / 60.0
  local minute = total_min % 60.0
  local hour = (total_min / 60.0) % 12.0

  ctx:circle(cx, cy, R, fr, fg, fb, fa)
  ctx:ring(cx, cy, R, 2.0, cr, cg, cb, ca)

  for i = 0, 11 do
    local a = (i / 12.0) * TAU
    local big = (i % 3 == 0)
    local inner = R - (big and 7.0 or 4.0)
    local outer = R - 2.0
    ctx:line(
      cx + math.cos(a) * inner, cy + math.sin(a) * inner,
      cx + math.cos(a) * outer, cy + math.sin(a) * outer,
      big and 2.0 or 1.0, cr, cg, cb, ca
    )
  end

  hand(ctx, cx, cy, hour, 12.0, R * 0.50, 3.0, cr, cg, cb, ca)
  hand(ctx, cx, cy, minute, 60.0, R * 0.72, 2.0, cr, cg, cb, ca)
  hand(ctx, cx, cy, sec, 60.0, R * 0.80, 1.0, ar, ag, ab, aa)
  ctx:circle(cx, cy, 2.0, ar, ag, ab, aa)

  if params.digital ~= false then
    local hh = math.floor((total_min / 60.0) % 12.0)
    if hh == 0 then hh = 12 end
    local mm = math.floor(minute)
    local label = string.format("%02d:%02d", hh, mm)
    local fs = math.max(9.0, R * 0.30)
    local tw = fs * 2.7
    ctx:round_rect(cx - tw * 0.5 - 3.0, cy + R * 0.38, tw + 6.0, fs + 4.0, 3.0, 0, 0, 0, 170)
    ctx:text_centered(cx, cy + R * 0.38 + (fs + 4.0) * 0.5, label, fs, cr, cg, cb, 255)
  end

  -- 指针点：黑底 + 主题色内点。系统箭头已被隐藏，没有描边时白点会与白墙/白窗口融为一体。
  local dr, dg, db, da = hex_to_rgb(params.dot_color or "#FFFFFF")
  local orr, org, orb, ora = hex_to_rgb(params.dot_outline or "#000000")
  local dsz = params.dot_size or 3
  ctx:circle(mouse_x or px, mouse_y or py, dsz + 1.5, orr, org, orb, ora)
  ctx:circle(mouse_x or px, mouse_y or py, dsz, dr, dg, db, da)
end
