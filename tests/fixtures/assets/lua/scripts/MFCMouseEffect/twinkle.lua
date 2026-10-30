-- MFCMouseEffect · Stardust Stream
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 星尘喷流：滚轮时沿滚动方向喷出 10–22 颗粒子，带阻力、湍流、闪烁与拖尾线；
-- 上游基础时长 220ms。

name = "Stardust Stream"
effect_name = "Stardust Stream"
kind = "scroll"
description = "MFCMouseEffect Stardust Stream：滚轮方向的星尘喷流。"

local MAX_BURSTS = 4
local bursts = {}

local function mix(a, b, t) return a * (1 - t) + b * t end

function schema()
  return {
    { key = "color",    label = "星尘颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "glow",     label = "辉光颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "size",     label = "尺寸倍数", type = "float", min = 0.5, max = 2.5, step = 0.1, default = 2.2 },
    { key = "count",    label = "粒子倍数", type = "float", min = 0.5, max = 2.0, step = 0.1, default = 1.5 },
    { key = "duration", label = "时长(ms)", type = "int",   min = 80,  max = 800, step = 10,  default = 400 },
  }
end

function init()
  bursts = {}
end

local function spawn_burst(x, y, dir, params)
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FF5AF7")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#FF5AF7")
  local dx, dy = math.cos(dir), math.sin(dir)
  local px, py = -dy, dx
  local burst_ms = math.max(80, params.duration or 400)
  local count = math.floor((10 + 12) * (params.count or 1.5))
  local parts = {}
  for _ = 1, count do
    local co = rng(-1, 1) * (math.pi / 6)
    local c, s = math.cos(co), math.sin(co)
    local sprayx = dx * c + px * s
    local sprayy = dy * c + py * s
    local sj = rng(-1, 1) * (1.4 + 3.0)
    local bj = rng(0, 1) * (2.5 + 5.0)
    local speed = 4.2 + rng(0, 1) * (4.8 + 2.2)
    parts[#parts + 1] = {
      x = x + px * sj - dx * bj,
      y = y + py * sj - dy * bj,
      prevx = x + px * sj - dx * bj,
      prevy = y + py * sj - dy * bj,
      vx = sprayx * speed,
      vy = sprayy * speed,
      size = 1.0 + rng(0, 1) * 1.4,
      age = 0,
      -- 上游寿命是时长的 0.43–0.98 倍；这里放大到 0.75–1.375 倍，默认 400ms 即 300–550ms，
      -- 且随时长滑条同比缩放——不会再出现“时长还没走完，粒子已经全死光”的空尾。
      life = burst_ms * 0.75 + rng(0, 1) * burst_ms * 0.625,
      flicker = rng(0, math.pi * 2),
    }
  end
  bursts[#bursts + 1] = {
    parts = parts,
    x = x, y = y,
    start = wall_ms or 0,
    duration = burst_ms,
    dirx = dx, diry = dy,
    size = params.size or 2.2,
    cr = cr, cg = cg, cb = cb, ca = ca,
    gr = gr, gg = gg, gb = gb, ga = ga,
  }
  while #bursts > MAX_BURSTS do table.remove(bursts, 1) end
end

function on_wheel(delta, horizontal, x, y, params)
  if delta == 0 then return end
  params = params or {}
  local dir
  if horizontal then
    dir = delta >= 0 and 0 or math.pi
  else
    dir = delta >= 0 and -math.pi / 2 or math.pi / 2
  end
  spawn_burst(x, y, dir, params)
end

function step(dt, t, params)
  local now = wall_ms or 0
  local f = math.min(2.0, dt * 60.0)
  for bi = #bursts, 1, -1 do
    local b = bursts[bi]
    if now - b.start >= b.duration then
      table.remove(bursts, bi)
    else
      local drag = math.pow(0.90, f)
      local flow = 0.015 * f
      local turb = 0.040 * f
      for i = #b.parts, 1, -1 do
        local p = b.parts[i]
        p.age = p.age + dt * 1000
        if p.age >= p.life then
          table.remove(b.parts, i)
        else
          p.prevx, p.prevy = p.x, p.y
          p.vx = p.vx * drag + b.dirx * flow + rng(-1, 1) * turb
          p.vy = p.vy * drag + b.diry * flow + rng(-1, 1) * turb
          p.x = p.x + p.vx * f
          p.y = p.y + p.vy * f
          p.flicker = p.flicker + 0.16 * f
        end
      end
    end
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  for bi = 1, #bursts do
    local b = bursts[bi]
    local t = clamp((now - b.start) / b.duration, 0, 1)
    local life_fade = 1 - clamp((t - 0.70) / 0.30, 0, 1)
    -- 起手闪光：滚轮的一瞬给一个明确的视觉锚点（上游是一簇粒子，短促且小）。
    local flash = 1 - clamp(t / 0.35, 0, 1)
    if flash > 0.01 then
      ctx:glow(b.x, b.y, 18.0 + 20.0 * (1.0 - flash),
        b.cr, b.cg, b.cb, math.floor(170 * flash * b.ca / 255))
    end
    if life_fade > 0.01 then
      local seconds = (now - b.start) / 1000.0
      local base_size = (0.78 + 1.20) * b.size
      local cr, cg, cb = b.cr / 255, b.cg / 255, b.cb / 255
      local gr, gg, gb = b.gr / 255, b.gg / 255, b.gb / 255
      for i = 1, #b.parts do
        local p = b.parts[i]
        local life = 1 - clamp(p.age / p.life, 0, 1)
        if life > 0 then
          local flicker = 0.86 + 0.14 * math.sin(seconds * 9.0 + p.flicker)
          local draw_size = base_size * p.size * (0.55 + 0.45 * life) * flicker
          -- ctx 的颜色 alpha 是 0–255（上游内部的 coreAlpha 是 0–1，乘回 255）。
          local core_a = 178.0 * life * life_fade * (b.ca / 255)
          local mix_t = 0.05 + (1 - life) * 0.14
          local c_r, c_g, c_b = mix(cr, 1, mix_t), mix(cg, 1, mix_t), mix(cb, 1, mix_t)
          local tr = mix(gr, c_r, 0.38)
          local tg = mix(gg, c_g, 0.38)
          local tbv = mix(gb, c_b, 0.38)
          ctx:line(p.prevx, p.prevy, p.x, p.y, math.max(0.75, draw_size * 0.26),
            math.floor(tr * 255), math.floor(tg * 255), math.floor(tbv * 255),
            math.floor(core_a * 0.64 * b.ga / 255))
          ctx:circle(p.x, p.y, draw_size * 0.61,
            math.floor(tr * 255), math.floor(tg * 255), math.floor(tbv * 255),
            math.floor(core_a * 0.14 * b.ga / 255))
          ctx:circle(p.x, p.y, draw_size * 0.5,
            math.floor(c_r * 255), math.floor(c_g * 255), math.floor(c_b * 255),
            math.floor(core_a))
        end
      end
    end
  end
end
