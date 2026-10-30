-- MFCMouseEffect · Energy Charge
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 蓄力（能量环）：按住 260ms 后出现；背景环 + 进度弧 + 外发光弧 + 头部圆点，
-- 整体呼吸。进度 = 按住时长 / progress_ms，抬起后淡出。默认进度满 900ms。

name = "Energy Charge"
effect_name = "Energy Charge"
kind = "hold"
description = "MFCMouseEffect Energy Charge：按住蓄力的能量进度环。"

local HOLD_DELAY = 260
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0

function schema()
  return {
    { key = "color",      label = "能量颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "glow",       label = "辉光颜色", type = "color", alpha = true, default = "#4DF8FF66" },
    { key = "size",       label = "圆环半径", type = "float", min = 20, max = 140, step = 2,   default = 70 },
    { key = "width",      label = "环宽",     type = "float", min = 1,  max = 8,   step = 0.1, default = 2.8 },
    { key = "duration",   label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",     label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
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

local function arc(ctx, cx, cy, radius, start_deg, sweep_deg, width, r, g, b, a)
  if sweep_deg <= 0.01 then return end
  local steps = math.max(6, math.floor(sweep_deg / 4.0))
  local prev_x, prev_y
  for i = 0, steps do
    local ang = math.rad(start_deg + sweep_deg * i / steps)
    local x = cx + math.cos(ang) * radius
    local y = cy + math.sin(ang) * radius
    if i > 0 then
      ctx:line(prev_x, prev_y, x, y, width, r, g, b, a)
    end
    prev_x, prev_y = x, y
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#4DF8FF66")
  local radius = params.size or 70
  local width = params.width or 2.8
  local now = wall_ms or 0
  local pulse = 0.5 + 0.5 * math.sin(now * 0.0045)
  local alpha = (0.55 + 0.45 * pulse) * vis
  local a_stroke = math.floor(ca * alpha)
  local a_glow = math.floor(ga * alpha * 0.6)

  ctx:ring(hx, hy, radius, width, r, g, b, math.floor(a_stroke * 0.25))
  arc(ctx, hx, hy, radius, -90, progress * 360, width + 8, gr, gg, gb, a_glow)
  arc(ctx, hx, hy, radius, -90, progress * 360, width + 1, r, g, b, a_stroke)

  local ang = math.rad(-90 + progress * 360)
  local dx, dy = hx + math.cos(ang) * radius, hy + math.sin(ang) * radius
  ctx:circle(dx, dy, width * 2.2, r, g, b, a_stroke)
end
