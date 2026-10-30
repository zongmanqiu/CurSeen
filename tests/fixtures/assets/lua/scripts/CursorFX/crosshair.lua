-- CursorFX · crosshair / 复刻自 cursorfx-main/src/cursors/crosshair.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：全屏准星。四条贯穿屏幕的虚线参考线在准星处断开，中心是圆环 + 四向刻度；
-- 按住时准星外扩、轻微上跳并抖动，模拟后坐力；右上角显示实时坐标。
--
-- 实现：宿主没有 canvas setLineDash，虚线用按间距排布的短 line 模拟。默认间距 28px，
-- 1920×1080 下约 107 条短线；关掉“虚线”后只剩 4 条长线。
--
-- 性能：默认约 120 条线 + 1 环 + 5 短线 + 1 圆点 + 1 圆角底板 + 1 行文字 ≈ 130 图元。

name = "crosshair"
effect_name = "crosshair"
kind = "cursor"
description = "CursorFX crosshair：带实时坐标与后坐力抖动的全屏准星。"
hide_cursor = true

function schema()
  return {
    { key = "color",  label = "颜色",   type = "color", default = "#00E5FF" },
    { key = "size",   label = "准星半径", type = "int", min = 6, max = 30, step = 1, default = 12 },
    { key = "coords", label = "显示坐标", type = "bool", default = true },
    { key = "dashed", label = "虚线参考线", type = "bool", default = true },
    { key = "recoil", label = "后坐力", type = "bool", default = true },
  }
end

local radius = 12
local kick = 0
local since_kick = 0

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  radius = 12
  kick = 0
  since_kick = 0
end

function on_mouse_down(button, x, y, params)
  if params.recoil ~= false then
    kick = 1.0
    since_kick = 0
  end
end

function step(dt, t, params)
  local f = clampf(dt * 60.0, 0.0, 2.0)
  since_kick = since_kick + dt
  if params.recoil ~= false and mouse_down and since_kick > 1.0 / 7.0 then
    kick = 1.0
    since_kick = 0
  end
  kick = kick * (0.82 ^ f)
  local base = (params.size or 12) + kick * 14.0
  radius = radius + (base - radius) * math.min(1.0, 0.35 * f)
end

local function dashed_line(ctx, x1, y1, x2, y2, spacing, dash, r, g, b, a)
  local dx, dy = x2 - x1, y2 - y1
  local len = math.sqrt(dx * dx + dy * dy)
  if len <= 0.001 then return end
  local ux, uy = dx / len, dy / len
  local pos = 0.0
  while pos < len do
    local e = math.min(pos + dash, len)
    ctx:line(x1 + ux * pos, y1 + uy * pos, x1 + ux * e, y1 + uy * e, 1.0, r, g, b, a)
    pos = pos + spacing
  end
end

function draw(ctx, params)
  local color = params.color or "#00E5FF"
  local cr, cg, cb, ca = hex_to_rgb(color)
  local sw, sh = screen_w or 1920, screen_h or 1080

  local jitter = kick * 1.5
  local x = (mouse_x or 0) + rng(-jitter, jitter)
  local y = (mouse_y or 0) - kick * 6.0 + rng(-jitter, jitter)
  local r = math.max(radius, 3.0)

  local ga = math.floor(0.55 * ca)
  local gap = r + 6.0
  local left_end = x - gap
  local right_start = x + gap
  local top_end = y - gap
  local bottom_start = y + gap

  if params.dashed ~= false then
    dashed_line(ctx, 0, y, left_end, y, 28.0, 12.0, cr, cg, cb, ga)
    dashed_line(ctx, right_start, y, sw, y, 28.0, 12.0, cr, cg, cb, ga)
    dashed_line(ctx, x, 0, x, top_end, 28.0, 12.0, cr, cg, cb, ga)
    dashed_line(ctx, x, bottom_start, x, sh, 28.0, 12.0, cr, cg, cb, ga)
  else
    ctx:line(0, y, left_end, y, 1.0, cr, cg, cb, ga)
    ctx:line(right_start, y, sw, y, 1.0, cr, cg, cb, ga)
    ctx:line(x, 0, x, top_end, 1.0, cr, cg, cb, ga)
    ctx:line(x, bottom_start, x, sh, 1.0, cr, cg, cb, ga)
  end

  ctx:ring(x, y, r, 1.5, cr, cg, cb, ca)
  ctx:line(x - r - 6, y, x - r + 4, y, 1.5, cr, cg, cb, ca)
  ctx:line(x + r - 4, y, x + r + 6, y, 1.5, cr, cg, cb, ca)
  ctx:line(x, y - r - 6, x, y - r + 4, 1.5, cr, cg, cb, ca)
  ctx:line(x, y + r - 4, x, y + r + 6, 1.5, cr, cg, cb, ca)
  ctx:circle(x, y, 1.6, cr, cg, cb, ca)

  if params.coords ~= false then
    local label = string.format("%d, %d", math.floor(x + 0.5), math.floor(y + 0.5))
    local w = #label * 6.6 + 10.0
    local lx = x + 14.0
    if lx + w > sw then lx = x - 14.0 - w end
    local ly = y + 12.0
    if ly + 18.0 > sh then ly = y - 30.0 end

    ctx:round_rect(lx, ly, w, 18.0, 4.0, 0, 0, 0, 153)
    ctx:text(lx + 5.0, ly + 3.0, label, 11.0, cr, cg, cb, 255)
  end
end
