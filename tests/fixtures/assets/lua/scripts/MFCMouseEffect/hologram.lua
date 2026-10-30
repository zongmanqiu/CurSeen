-- MFCMouseEffect · Hologram (3D)
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 全息投影（3D）：50° 倾斜的地面分段环 + 30 个上升粒子（带地面投影线）+ 中心光晕。
-- 按住 260ms 后出现，地面环随进度扩大。

name = "Hologram (3D)"
effect_name = "Hologram (3D)"
kind = "hold"
description = "MFCMouseEffect Hologram：按住时升起的倾斜全息环。"

local HOLD_DELAY = 260
local PARTICLES = 30
local TILT = math.rad(50)
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0
local particles = {}
local started = false

local function project(x, y, z)
  local cy = math.cos(TILT)
  local sy = math.sin(TILT)
  local y2 = y * cy - z * sy
  local z2 = y * sy + z * cy
  local scale = 400.0 / (400.0 + z2)
  return hx + x * scale, hy + y2 * scale
end

function schema()
  return {
    { key = "color",    label = "全息颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "size",     label = "投影半径", type = "float", min = 20, max = 160, step = 2,   default = 70 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 6,   step = 0.1, default = 2.8 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
  }
end

local function make_particles(radius)
  particles = {}
  for _ = 1, PARTICLES do
    local angle = rng(0, math.pi * 2)
    local r = radius * (0.2 + rng(0, 0.8))
    particles[#particles + 1] = {
      x = r * math.cos(angle),
      y = 0,
      z = r * math.sin(angle),
      speed = rng(0.5, 2.48),
    }
  end
  started = true
end

function init()
  press_at = nil
  vis = 0
  progress = 0
  hx, hy = mouse_x or 0, mouse_y or 0
  particles = {}
  started = false
end

function on_mouse_down(button, x, y, params)
  press_at = wall_ms or 0
  hx, hy = x, y
  started = false
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
      if not started then make_particles((params.size or 70)) end
    end
  else
    vis = vis - vis * math.min(1.0, dt * 8.0)
    if vis < 0.01 then vis = 0 end
  end

  if started then
    local base = params.size or 70
    for i = 1, #particles do
      local p = particles[i]
      p.y = p.y + p.speed * 2.0 * (dt * 60.0)
      if p.y < -base * 1.5 then
        p.y = 0
        local angle = rng(0, math.pi * 2)
        local r = base * (0.2 + rng(0, 0.8))
        p.x = r * math.cos(angle)
        p.z = r * math.sin(angle)
        p.speed = rng(0.5, 2.48)
      end
    end
  end
end

local function seg_ring(ctx, radius, spin, width, seg_count, gap, alpha, r, g, b)
  local step = math.pi * 2 / seg_count
  for i = 0, seg_count - 1 do
    local start_a = i * step + spin
    local sweep = step * (1.0 - gap)
    local prev_x, prev_y
    for j = 0, 10 do
      local ang = start_a + sweep * j / 10
      local x, y = project(radius * math.cos(ang), 0, radius * math.sin(ang))
      if j > 0 then ctx:line(prev_x, prev_y, x, y, width, r, g, b, alpha) end
      prev_x, prev_y = x, y
    end
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local base = params.size or 70
  local width = params.width or 2.8
  local seconds = (wall_ms or 0) / 1000.0
  local main = base * (0.3 + 0.7 * progress)
  local alpha = math.floor(ca * vis * (1.0 - progress * 0.5))

  -- 地面双环线宽接回用户参数：上游比例 宽4/宽2.8（外环 1.43×、内环 1.0×），
  -- 默认值下与原实现完全一致；此前 width 只被已删除的中心光晕消费。
  seg_ring(ctx, main, seconds * 0.5, width * 1.43, 3, 0.2, alpha, r, g, b)
  seg_ring(ctx, main * 0.7, -seconds, width, 4, 0.4, math.floor(alpha * 0.8), r, g, b)

  local p_alpha = math.floor(ca * 0.7 * vis)
  for i = 1, #particles do
    local p = particles[i]
    local x, y = project(p.x, p.y, p.z)
    local gx, gy = project(p.x, 0, p.z)
    local scale = 400.0 / (400.0 + p.z)
    local size = math.max(0.5, 3.0 * scale)
    ctx:circle(x, y, size, r, g, b, p_alpha)
    ctx:line(x, y, gx, gy, 1.0, r, g, b, math.floor(p_alpha * 0.3))
  end

  local cx, cy = project(0, 0, 0)
  -- 中心光晕：上游为双 stop 全透明（HologramHudRenderer 中心不可见），不画。
  _ = cx
end
