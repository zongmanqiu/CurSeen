-- MFCMouseEffect · Helix Suspension
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 螺旋悬浮：静止 1500ms 后，3 条 20 节点链条按 2 圈螺旋排布绕光标缓慢旋转，
-- 节点用径向渐变发光。上游 id 也是 tubes（Hover 通道），本包用 tubes_hover 区分。

name = "Helix Suspension"
effect_name = "Helix Suspension"
kind = "hover"
description = "MFCMouseEffect Helix Suspension：静止后绕光标旋转的三条光链。"

local IDLE_MS = 1500
local NODES = 20
local last_activity = 0
local vis = 0.0
local chains = {}

local COLORS = {
  { 249, 103, 251 },
  { 83, 188, 40 },
  { 105, 88, 213 },
}

local function build()
  chains = {}
  local scales = { 0.8, 1.0, 1.2 }
  for c = 1, 3 do
    chains[c] = {}
    for i = 0, NODES - 1 do
      local angle = i / NODES * math.pi * 2 * 2
      local rad = (10.0 + i * 2.0) * scales[c]
      chains[c][i + 1] = {
        angle = angle,
        rad = rad,
        phase = angle,
      }
    end
  end
end

function schema()
  return {
    { key = "rainbow",  label = "彩虹色",   type = "bool",  default = false },
    { key = "size",     label = "尺寸倍数", type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
    { key = "speed",    label = "旋转速度", type = "float", min = 0.2, max = 2.5, step = 0.1,  default = 1.0 },
    { key = "opacity",  label = "不透明度", type = "float", min = 0.05, max = 1,  step = 0.01, default = 0.78 },
  }
end

local function activity()
  last_activity = wall_ms or 0
  vis = 0.0
end

function init()
  last_activity = wall_ms or 0
  vis = 0.0
  build()
end

function on_move(x, y, t, params) activity() end
function on_mouse_down(button, x, y, params) activity() end
function on_mouse_up(button, x, y, params) activity() end
function on_wheel(delta, horizontal, x, y, params) activity() end

function step(dt, t, params)
  local now = wall_ms or 0
  local target = 0.0
  if not mouse_down and now - last_activity >= IDLE_MS then target = 1.0 end
  vis = vis + (target - vis) * math.min(1.0, dt * 4.0)
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  local scale = params.size or 1.0
  local speed = params.speed or 1.0
  local alpha_base = (params.opacity or 0.78) * vis
  local time = (elapsed or 0) * 1000.0 * speed
  local speeds = { 0.002, 0.003, 0.0015 }

  for c = 1, 3 do
    local col
    if params.rainbow then
      local hue = ((elapsed or 0) * 200.0 + (c - 1) * 120.0) % 360
      local hr, hg, hb = hsv_to_rgb(hue, 0.9, 0.6)
      col = { hr, hg, hb }
    else
      col = COLORS[c]
    end
    local chain = chains[c]
    if chain then
      local inv = 1.0 / NODES
      for i = 1, NODES do
        local node = chain[i]
        local rot = time * speeds[c] + node.phase
        local cur_r = node.rad * scale
        cur_r = cur_r + math.sin(time * 2.0 + i * 0.1) * 5.0 * scale
        local x = mx + math.cos(rot) * cur_r
        local y = my + math.sin(rot) * cur_r
        local ratio = 1.0 - (i - 1) * inv
        local radius = (3.0 + 10.0 * ratio) * scale
        local alpha = math.floor(200 * alpha_base)
        if alpha > 0 and radius > 0.3 then
          local cr2 = math.min(255, col[1] + 150)
          local cg2 = math.min(255, col[2] + 150)
          local cb2 = math.min(255, col[3] + 150)
          ctx:radial_stops(x, y, radius, {
            { offset = 0.0, r = cr2, g = cg2, b = cb2, a = alpha },
            { offset = 0.55, r = col[1], g = col[2], b = col[3], a = math.floor(alpha * 0.55) },
            { offset = 1.0, r = col[1], g = col[2], b = col[3], a = 0 },
          })
        end
      end
    end
  end
end
