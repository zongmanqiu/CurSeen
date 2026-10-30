-- MFCMouseEffect · Lightning
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 闪电（奇点）：按住 260ms 后出现；中心光球随进度变大，24 条径向闪电向外
-- 伸缩。闪电角度在本次按住开始时确定，逐帧只抖动位置。

name = "Lightning"
effect_name = "Lightning"
kind = "hold"
description = "MFCMouseEffect Lightning：按住时脉动的球形闪电。"

local HOLD_DELAY = 260
local BOLTS = 24
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0
local bolts = {}

function schema()
  return {
    { key = "color",    label = "闪电颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "glow",     label = "辉光颜色", type = "color", alpha = true, default = "#4DF8FF66" },
    { key = "size",     label = "闪电半径", type = "float", min = 20, max = 160, step = 2,   default = 70 },
    { key = "width",    label = "核心宽度", type = "float", min = 0.5,max = 8,   step = 0.1, default = 2.8 },
    { key = "bolts",    label = "闪电数量", type = "int",   min = 6,  max = 40,  step = 2,   default = 24 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
  }
end

local function make_bolts(n)
  bolts = {}
  for i = 1, n do
    bolts[i] = {
      angle = rng(0, math.pi * 2),
      speed = rng(1.0, 3.0),
      phase = rng(0, 1),
      offset = rng(0, 5),
    }
  end
end

function init()
  press_at = nil
  vis = 0
  progress = 0
  hx, hy = mouse_x or 0, mouse_y or 0
  make_bolts(BOLTS)
end

function on_mouse_down(button, x, y, params)
  press_at = wall_ms or 0
  hx, hy = x, y
  make_bolts(math.floor((params and params.bolts) or BOLTS))
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

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#4DF8FF66")
  local radius = params.size or 70
  local width = params.width or 2.8
  local now = wall_ms or 0
  local core_radius = width * (2.0 + progress * 4.0)
  local pulse = 0.5 + 0.5 * math.sin(now * 0.05)
  local core_alpha = (0.8 + 0.2 * pulse) * vis

  -- 中心外光晕与能量球
  ctx:radial_stops(hx, hy, core_radius * 2.5, {
    { offset = 0.0, r = gr, g = gg, b = gb, a = math.floor(ga * 0.6 * vis) },
    { offset = 0.55, r = gr, g = gg, b = gb, a = math.floor(ga * 0.20 * vis) },
    { offset = 1.0, r = gr, g = gg, b = gb, a = 0 },
  })
  ctx:radial_stops(hx, hy, core_radius * 1.5, {
    { offset = 0.0, r = math.min(255, r + 100), g = math.min(255, g + 100),
      b = math.min(255, b + 100), a = math.floor(core_alpha * 255) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })

  local seconds = now / 1000.0
  local bolt_a = math.floor(ca * 0.7 * vis)
  for i = 1, math.min(#bolts, math.floor(params.bolts or BOLTS)) do
    local bolt = bolts[i]
    local linear = (seconds * bolt.speed + bolt.phase) % 1.0
    local dist_factor = 1.0 - linear
    local current = radius * 0.3 + radius * 0.8 * dist_factor + bolt.offset * 10.0
    if current > radius then current = radius end
    local length = 15.0 * (1 + bolt.speed)
    local x1 = hx + math.cos(bolt.angle) * current
    local y1 = hy + math.sin(bolt.angle) * current
    local x2 = hx + math.cos(bolt.angle) * (current + length)
    local y2 = hy + math.sin(bolt.angle) * (current + length)
    local jitter = rng(-1, 1)
    ctx:line(x1 + jitter, y1 + jitter, x2, y2, width * 0.5, r, g, b, bolt_a)
  end
end
