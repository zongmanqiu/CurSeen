-- MFCMouseEffect · Hex Shield
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 六边形（护盾）：按住 260ms 后，3 层同心六边形以不同速度反向旋转并随进度放大；
-- 第一层半透明填充，顶点各有发光点。整体呼吸。

name = "Hex Shield"
effect_name = "Hex Shield"
kind = "hold"
description = "MFCMouseEffect Hex Shield：按住时展开的三层旋转六边形护盾。"

local HOLD_DELAY = 260
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0

function schema()
  return {
    { key = "color",    label = "线条颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "fill",     label = "填充颜色", type = "color", alpha = true, default = "#00000000" },
    { key = "glow",     label = "顶点光点", type = "color", alpha = true, default = "#4DF8FF66" },
    { key = "size",     label = "护盾半径", type = "float", min = 20, max = 160, step = 2,   default = 70 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 6,   step = 0.1, default = 2.8 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
  }
end

function init()
  press_at = nil
  vis = 0
  progress = 0
  hx, hy = mouse_x or 0, mouse_y or 0
end

function on_mouse_down(button, x, y, params)
  press_at = wall_ms or 0
  hx, hy = x, y
end

function on_mouse_up(button, x, y, params)
  press_at = nil
end

function step(dt, t, params)
  local now = wall_ms or 0
  local k = math.min(1.0, (params.follow or 0.25) * dt * 60.0)
  hx = hx + ((mouse_x or hx) - hx) * k
  hy = hy + ((mouse_y or hy) - hy) * k
  if press_at and mouse_down then
    local held = now - press_at
    if held >= HOLD_DELAY then
      progress = clamp(held / math.max(300, params.duration or 900), 0, 1)
      vis = vis + (1 - vis) * math.min(1.0, dt * 10.0)
    end
  else
    vis = vis - vis * math.min(1.0, dt * 8.0)
    if vis < 0.01 then vis = 0 end
  end
end

local function hex_points(cx, cy, radius, rot)
  local pts = {}
  for i = 0, 5 do
    local a = rot + i * (math.pi / 3)
    pts[i + 1] = { cx + math.cos(a) * radius, cy + math.sin(a) * radius }
  end
  return pts
end

local function draw_hex(ctx, radius, rot, width, alpha_mul, fill, params, colors)
  if radius <= 1.0 then return end
  local pts = hex_points(hx, hy, radius, rot)
  if fill then
    ctx:polygon(pts, colors.fr, colors.fg, colors.fb,
      math.floor(colors.fa * alpha_mul * 0.3))
  end
  for i = 1, 6 do
    local a = pts[i]
    local b = pts[i % 6 + 1]
    ctx:line(a[1], a[2], b[1], b[2], width, colors.r, colors.g, colors.b,
      math.floor(colors.ca * alpha_mul))
  end
  local dot_r = width * 1.5
  for i = 1, 6 do
    local p = pts[i]
    ctx:circle(p[1], p[2], dot_r, colors.gr, colors.gg, colors.gb,
      math.floor(colors.ga * alpha_mul))
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local fr, fg, fb, fa = hex_to_rgb(params.fill or "#00000000")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#4DF8FF66")
  local radius = params.size or 70
  local width = params.width or 2.8
  local now = wall_ms or 0
  local seconds = now / 1000.0
  local pulse = 0.5 + 0.5 * math.sin(now * 0.003)
  local alpha_base = (0.6 + 0.4 * pulse) * vis
  local colors = {
    r = r, g = g, b = b, ca = ca,
    fr = fr, fg = fg, fb = fb, fa = fa,
    gr = gr, gg = gg, gb = gb, ga = ga,
  }

  draw_hex(ctx, radius * 0.4 * progress, seconds * 2.0, width,
    alpha_base, true, params, colors)
  draw_hex(ctx, radius * 0.7 * progress, -seconds * 0.5, width * 0.8,
    alpha_base * 0.7, false, params, colors)
  draw_hex(ctx, radius * 1.0 * progress, seconds + 0.5, width * 0.5,
    alpha_base * 0.4, false, params, colors)
end
