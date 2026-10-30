-- MFCMouseEffect · Meteor Shower
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 绚丽流星：锥形尾迹 + 头部白芯 + 火花 + 多层头部光晕 + 运动方向 flare。
-- 上游默认历史 520ms、sparkRate 1、sparkSpeed 1。

name = "Meteor Shower"
effect_name = "Meteor Shower"
kind = "trail"
description = "MFCMouseEffect Meteor Shower：带火花与头部光晕的流星拖尾。"

local MAX_POINTS = 80
local MAX_SPARKS = 220
local pts = {}
local sparks = {}
local last_ms = 0

function schema()
  return {
    { key = "color",        label = "流尾颜色", type = "color", alpha = true, default = "#64FFDA" },
    { key = "width",        label = "线宽",     type = "float", min = 1, max = 16, step = 0.5, default = 4 },
    { key = "duration",     label = "历史时长(ms)", type = "int", min = 100, max = 2000, step = 10, default = 520 },
    { key = "spark_rate",   label = "火花频率", type = "float", min = 0.2, max = 3, step = 0.1, default = 1.0 },
    { key = "spark_speed",  label = "火花速度", type = "float", min = 0.2, max = 3, step = 0.1, default = 1.0 },
    { key = "opacity",      label = "不透明度", type = "float", min = 0.05, max = 1, step = 0.01, default = 0.92 },
  }
end

function init()
  pts = {}
  sparks = {}
  last_ms = 0
end

function on_move(x, y, t, params)
  local now = wall_ms or 0
  last_ms = now
  pts[#pts + 1] = { x = x, y = y, at = now }
  while #pts > MAX_POINTS do table.remove(pts, 1) end

  if #pts >= 2 then
    local prev = pts[#pts - 1]
    local dx, dy = x - prev.x, y - prev.y
    local dist = math.sqrt(dx * dx + dy * dy)
    if dist > 1.0 then
      local rate = params.spark_rate or 1.0
      local n = math.floor(math.min(4, dist / 7.0 + 1) * rate)
      if n < 1 then n = 1 end
      if n > 8 then n = 8 end
      local base = math.atan2(-dy, -dx)
      for _ = 1, n do
        if #sparks >= MAX_SPARKS then break end
        local a = base + rng(-0.55, 0.55)
        local sp = rng(0.6, 3.2) * (params.spark_speed or 1.0)
        sparks[#sparks + 1] = {
          x = x, y = y,
          vx = math.cos(a) * sp,
          vy = math.sin(a) * sp,
          life = 1.0,
          size = rng(1.0, 3.5),
        }
      end
    end
  end
end

function step(dt, t, params)
  local f = math.min(2.0, dt * 60.0)
  local now = wall_ms or 0
  local dur = params.duration or 520
  while #pts > 0 and now - pts[1].at > dur do table.remove(pts, 1) end

  for i = #sparks, 1, -1 do
    local s = sparks[i]
    s.x = s.x + s.vx * f
    s.y = s.y + s.vy * f
    s.vy = s.vy + 0.05 * f
    s.life = s.life - dt * 1.9
    if s.life <= 0 then table.remove(sparks, i) end
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  local n = #pts
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDA")
  local dur = math.max(100, params.duration or 520)
  local width = params.width or 4
  local base_a = (params.opacity or 0.92) * a
  local width_scale = math.max(0.35, width / 4.0)
  local head = n > 0 and pts[n] or nil
  -- 静止加速淡出（上游 MeteorRenderer.h idleFade 50/260）：停止移动 260ms 后
  -- 在 50ms 内把尾迹/火花/头部淡掉，旧版停住后尾迹多存活约 300ms。
  local since = now - last_ms
  local idle_fade = 1.0
  if since > 260 then idle_fade = clamp(1 - (since - 260) / 50, 0, 1) end

  if n >= 2 then
    for i = 1, n - 1 do
      local p1 = pts[i]
      local p2 = pts[i + 1]
      local ratio = (i - 1) / n
      local life = clamp(1 - (now - p1.at) / dur, 0, 1)
      if life > 0 then
        local w = (1.0 + 12.0 * ratio * life) * width_scale
        local alpha = math.floor(base_a * idle_fade * clamp(180.0 * ratio * life / 255.0, 0, 1))
        -- 流星识别：暖偏色，同时尊重用户配置的基色。
        local cr = math.min(255, r + 24)
        local cg = math.min(255, g + 18)
        local cb = math.max(100, b)
        ctx:line(p1.x, p1.y, p2.x, p2.y, math.max(1.0, w), cr, cg, cb, alpha)
        if ratio > 0.6 then
          local core_a = math.floor(alpha * 0.8)
          ctx:line(p1.x, p1.y, p2.x, p2.y, math.max(1.0, w * 0.3),
            255, 255, 255, core_a)
        end
      end
    end
  end

  for i = 1, #sparks do
    local s = sparks[i]
    local alpha = math.floor(255 * clamp(s.life, 0, 1) * base_a * idle_fade / 255)
    local size = math.max(0.4, s.size)
    ctx:circle(s.x, s.y, size, 255, 235, 170, alpha)
    if s.life > 0.5 then
      ctx:radial_stops(s.x, s.y, size * 2.5, {
        { offset = 0.0, r = 255, g = 235, b = 170, a = math.floor(alpha * 0.25) },
        { offset = 1.0, r = 255, g = 235, b = 170, a = 0 },
      })
    end
  end

  if head and idle_fade > 0 then
    -- 三层头部光晕：外壳柔、内芯亮、中心纯白。
    ctx:radial_stops(head.x, head.y, 18.0, {
      { offset = 0.0, r = 255, g = 200, b = 120, a = math.floor(40 * base_a * idle_fade / 255) },
      { offset = 1.0, r = 255, g = 200, b = 120, a = 0 },
    })
    ctx:radial_stops(head.x, head.y, 10.0, {
      { offset = 0.0, r = 255, g = 240, b = 180, a = math.floor(110 * base_a * idle_fade / 255) },
      { offset = 1.0, r = 255, g = 240, b = 180, a = 0 },
    })
    ctx:circle(head.x, head.y, 4.0, 255, 255, 255, math.floor(255 * idle_fade))

    if n >= 2 and idle_fade > 0.4 then
      local prev = pts[n - 1]
      local dx, dy = head.x - prev.x, head.y - prev.y
      local len = math.sqrt(dx * dx + dy * dy)
      if len > 0.5 then
        local ux, uy = dx / len, dy / len
        local px, py = -uy, ux
        local a_flare = math.floor(120 * base_a / 255)
        ctx:line(head.x - ux * 16, head.y - uy * 16,
          head.x + ux * 5.6, head.y + uy * 5.6, 1.2, 255, 255, 255, a_flare)
        ctx:line(head.x - px * 8, head.y - py * 8,
          head.x + px * 8, head.y + py * 8, 1.2, 255, 255, 255, a_flare)
      end
    end
  end
end
