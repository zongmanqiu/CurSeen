-- MFCMouseEffect · Sci-Fi Tubes
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 科幻管道：3 条 30 节点链条以不同滞后跟随指针，节点用径向渐变画成发光管珠；
-- 静止后链条收敛并整体淡出。上游链条颜色取自基色向亮/绿/蓝偏移三种色调。

name = "Sci-Fi Tubes"
effect_name = "Sci-Fi Tubes"
kind = "trail"
description = "MFCMouseEffect Sci-Fi Tubes：三条滞后跟随的发光管链。"

local CHAINS = 3
local NODES = 30
local MIN_SEG = 3.5
local chains = {}
local inited = false
local last_move = 0
local fade = 1.0

local function base_tints(r, g, b)
  return {
    { math.min(255, r + 34), math.max(0, g - 12), math.min(255, b + 20) },
    { math.max(0, r - 16), math.min(255, g + 28), math.max(0, b - 14) },
    { math.min(255, r + 8), math.min(255, g + 4), math.min(255, b + 34) },
  }
end

function schema()
  return {
    { key = "color",    label = "管道基色", type = "color", alpha = true, default = "#64FFDA" },
    { key = "nodes",    label = "节点数",   type = "int",   min = 10, max = 30, step = 2, default = 30 },
    { key = "lag",      label = "跟随滞后", type = "float", min = 0.1, max = 1.0, step = 0.05, default = 1.0 },
    { key = "opacity",  label = "不透明度", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.9 },
  }
end

function init()
  chains = {}
  for c = 1, CHAINS do
    chains[c] = {}
    for i = 1, NODES do chains[c][i] = { x = 0, y = 0 } end
  end
  inited = false
  last_move = 0
  fade = 1.0
end

local function reset_at(x, y)
  for c = 1, CHAINS do
    for i = 1, NODES do
      chains[c][i].x = x
      chains[c][i].y = y
    end
  end
  inited = true
  last_move = wall_ms or 0
  fade = 1.0
end

function on_move(x, y, t, params)
  if not inited then reset_at(x, y) end
  last_move = wall_ms or 0
  fade = 1.0
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not inited then reset_at(mx, my) end
  local f = math.min(2.0, dt * 60.0)
  local lags = { 0.30, 0.40, 0.50 }
  local multi = (params.lag or 1.0)

  for c = 1, CHAINS do
    local chain = chains[c]
    local lag = clamp(lags[c] * multi, 0.01, 1.0)
    local head = chain[1]
    local kh = 1.0 - math.pow(1.0 - lag, f)
    head.x = head.x + (mx - head.x) * kh
    head.y = head.y + (my - head.y) * kh
    for i = 2, #chain do
      local cnode = chain[i]
      local prev = chain[i - 1]
      local kn = 1.0 - math.pow(1.0 - lag, f)
      local nx = cnode.x + (prev.x - cnode.x) * kn
      local ny = cnode.y + (prev.y - cnode.y) * kn
      local dx, dy = prev.x - cnode.x, prev.y - cnode.y
      local dist = math.sqrt(dx * dx + dy * dy)
      if dist < MIN_SEG and dist > 0.01 then
        nx = prev.x - dx / dist * MIN_SEG
        ny = prev.y - dy / dist * MIN_SEG
      end
      cnode.x, cnode.y = nx, ny
    end
  end

  local idle = (wall_ms or 0) - last_move
  if idle > 50 then
    -- 静止淡出同上游 TubesRenderer.h：不再用固定 320ms 线性，而是看链条是否
    -- 「收敛」——链头距指针 ≤10px 视为已追上，每帧 -35（快淡），否则每帧 -10
    -- （慢淡）；0–255 口径折算到 0–1。
    local converged = true
    for c = 1, CHAINS do
      local h = chains[c][1]
      local ddx, ddy = h.x - mx, h.y - my
      if ddx * ddx + ddy * ddy > 100 then converged = false break end
    end
    local rate = (converged and 35.0 or 10.0) / 255.0
    fade = clamp(fade - rate * f, 0, 1)
  else
    fade = 1.0
  end
end

function draw(ctx, params)
  if not inited or fade <= 0.01 then return end
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDA")
  local tints = base_tints(r, g, b)
  local base_a = (params.opacity or 0.9) * a
  local count = math.floor(params.nodes or NODES)
  local frame_time = (wall_ms or 0) * 0.005
  local motion = clamp((mouse_speed or 0) / 24.0, 0, 1)
  local amplitude = 8.0 * (0.6 + motion * 0.8) * fade

  for c = 1, CHAINS do
    local chain = chains[c]
    local col = tints[c]
    local chain_phase = (c - 1) * (math.pi * 2 / 3)
    for i = count, 1, -1 do
      local node = chain[i]
      if node then
        local ratio = clamp(1.0 - (i - 1) / count, 0, 1)
        local radius = (2.0 + 7.0 * ratio) * fade
        if radius > 0.4 then
          local node_phase = (i - 1) * 0.3
          local x = node.x + math.cos(frame_time + node_phase + chain_phase) * amplitude
          local y = node.y + math.sin(frame_time + node_phase + chain_phase) * amplitude
          local alpha = math.floor(base_a * ratio * fade)
          local cr = math.min(255, col[1] + 100)
          local cg = math.min(255, col[2] + 100)
          local cb = math.min(255, col[3] + 100)
          ctx:radial_stops(x, y, radius, {
            { offset = 0.0, r = cr, g = cg, b = cb, a = alpha },
            { offset = 0.6, r = col[1], g = col[2], b = col[3], a = math.floor(alpha * 0.5) },
            { offset = 1.0, r = col[1], g = col[2], b = col[3], a = 0 },
          })
        end
      end
    end
  end
end
