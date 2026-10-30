-- MFCMouseEffect · Star
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击星星：10 顶点星形（外径 = 扩张半径，内径 = 0.4 倍），填充 + 描边一起淡出。
-- 按住 ≥260ms 视为长按，不触发本次点击。

name = "Star"
effect_name = "Star"
kind = "click"
description = "MFCMouseEffect Star：点击点扩张淡出的十角星。"

local HOLD_SUPPRESS_MS = 260
local MAX_STARS = 24
local stars = {}
local press_ms = 0

function schema()
  return {
    { key = "fill_color",   label = "填充颜色", type = "color", alpha = true, default = "#FFD700" },
    { key = "stroke_color", label = "描边颜色", type = "color", alpha = true, default = "#FF8C00" },
    { key = "start_radius", label = "起始半径", type = "float", min = 1,  max = 40,  step = 1,   default = 5 },
    { key = "end_radius",   label = "结束半径", type = "float", min = 10, max = 140, step = 1,   default = 35 },
    { key = "width",        label = "描边宽度", type = "float", min = 0.5,max = 8,   step = 0.1, default = 2.0 },
    { key = "duration",     label = "时长(ms)", type = "int",   min = 80, max = 1200,step = 10,  default = 350 },
  }
end

function init()
  stars = {}
  press_ms = 0
end

function on_mouse_down(button, x, y, params)
  press_ms = wall_ms or 0
end

function on_mouse_up(button, x, y, params)
  if (wall_ms or 0) - press_ms >= HOLD_SUPPRESS_MS then return end
  local fr, fg, fb, fa = hex_to_rgb(params.fill_color or "#FFD700")
  local sr, sg, sb, sa = hex_to_rgb(params.stroke_color or "#FF8C00")
  stars[#stars + 1] = {
    x = x, y = y,
    start = wall_ms or 0,
    duration = math.max(80, params.duration or 350),
    start_r = params.start_radius or 5,
    end_r = params.end_radius or 35,
    width = params.width or 2.0,
    fr = fr, fg = fg, fb = fb, fa = fa,
    sr = sr, sg = sg, sb = sb, sa = sa,
  }
  while #stars > MAX_STARS do table.remove(stars, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  for i = #stars, 1, -1 do
    if now - stars[i].start >= stars[i].duration then table.remove(stars, i) end
  end
end

local function star_points(cx, cy, radius, inner)
  local pts = {}
  for i = 0, 9 do
    local angle = i * math.pi / 5 - math.pi / 2
    local r = (i % 2 == 0) and radius or inner
    pts[i + 1] = { cx + r * math.cos(angle), cy + r * math.sin(angle) }
  end
  return pts
end

function draw(ctx, params)
  local now = wall_ms or 0
  for i = 1, #stars do
    local s = stars[i]
    local t = clamp((now - s.start) / s.duration, 0, 1)
    local eased = 1 - (1 - t) * (1 - t) * (1 - t)
    local alpha = 1 - eased
    local radius = s.start_r + (s.end_r - s.start_r) * eased
    if radius < 0.5 then radius = 0.5 end
    local pts = star_points(s.x, s.y, radius, radius * 0.4)
    ctx:polygon(pts, s.fr, s.fg, s.fb, math.floor(s.fa * alpha))
    for k = 1, 10 do
      local a = pts[k]
      local b = pts[k % 10 + 1]
      ctx:line(a[1], a[2], b[1], b[2], s.width, s.sr, s.sg, s.sb,
        math.floor(s.sa * alpha))
    end
  end
end
