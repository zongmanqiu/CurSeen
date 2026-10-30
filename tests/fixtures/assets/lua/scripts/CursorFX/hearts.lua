-- CursorFX · hearts
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：心形从按下点向上飘，左右摇摆、带一点重力，最后缩小淡出。心形用四次
-- 三次贝塞尔曲线采样成多边形（CurSeen 没有旋转图形原语，采样后直接喂 polygon）。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。

name = "hearts"
effect_name = "hearts"
kind = "click"
description = "CursorFX hearts：向上飘散、摇摆淡出的心形。"

local MAX_HEARTS = 160
local HEART_COLORS = { "#FF4D6D", "#FF8FAB", "#FF5C8A", "#FFB3C6", "#E63946" }

local hearts = {}

-- 单位心形（与 CursorFX util.heart 的四段贝塞尔同形），只算一次。
local HEART = {}
do
  local function segment(out, x0, y0, x1, y1, x2, y2, x3, y3, n)
    for i = 1, n do
      local u = i / n
      local v = 1.0 - u
      local a = v * v * v
      local b = 3.0 * v * v * u
      local c = 3.0 * v * u * u
      local d = u * u * u
      out[#out + 1] = {
        a * x0 + b * x1 + c * x2 + d * x3,
        a * y0 + b * y1 + c * y2 + d * y3,
      }
    end
  end

  segment(HEART, 0, 0.35, 0, 0, -0.5, -0.1, -0.5, -0.35, 5)
  segment(HEART, -0.5, -0.35, -0.5, -0.6, -0.15, -0.65, 0, -0.4, 5)
  segment(HEART, 0, -0.4, 0.15, -0.65, 0.5, -0.6, 0.5, -0.35, 5)
  segment(HEART, 0.5, -0.35, 0.5, -0.1, 0, 0, 0, 0.35, 5)
end

function schema()
  return {
    { key = "color",   label = "单色",     type = "color", alpha = true, default = "#FF4D6D" },
    { key = "rainbow", label = "原版配色", type = "bool",  default = true },
    { key = "count",   label = "心形数",   type = "int",   min = 2,  max = 40, step = 1,   default = 12 },
    { key = "size",    label = "心形大小", type = "float", min = 4,  max = 32, step = 0.5, default = 14 },
  }
end

function init()
  hearts = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

function on_press(x, y, t, params)
  local n = math.floor(params.count or 12)
  if n < 1 then n = 1 elseif n > 40 then n = 40 end
  local base = params.size or 14
  local rainbow = params.rainbow ~= false
  local fixed = params.color or "#FF4D6D"

  for i = 1, n do
    local life = rng(1.0, 1.83)
    hearts[#hearts + 1] = {
      x = x + rng(-10, 10),
      y = y + rng(-6, 6),
      vx = rng(-90, 90),
      vy = rng(-210, -72),
      s = base * rng(0.55, 1.4),
      rot = rng(-0.5, 0.5),
      ph = rng(0, 6.2831853),
      life = life,
      max = life,
      color = rainbow and HEART_COLORS[rng_int(1, #HEART_COLORS)] or fixed,
    }
  end

  while #hearts > MAX_HEARTS do
    table.remove(hearts, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local now = t or elapsed or 0
  local damp = (0.98) ^ (ds * 60.0)
  local n = #hearts
  local w = 0
  for i = 1, n do
    local p = hearts[i]
    p.x = p.x + (p.vx + math.sin(now * 4.0 + p.ph) * 36.0) * ds
    p.y = p.y + p.vy * ds
    p.vy = p.vy + 36.0 * ds
    p.vx = p.vx * damp
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      hearts[w] = p
    end
  end
  for i = #hearts, w + 1, -1 do
    hearts[i] = nil
  end
end

function draw(ctx, params)
  if #hearts == 0 then return end

  for i = 1, #hearts do
    local p = hearts[i]
    local t = p.life / p.max
    local al = a8(255, math.min(1.0, t * 2.0))
    if al > 0 then
      local r, g, b, ca = hex_to_rgb(p.color)
      local s = p.s
      if t > 0.85 then
        s = s * (1.0 + (t - 0.85) * 2.0)
      end
      local cr, sr = math.cos(p.rot), math.sin(p.rot)
      local pts = {}
      for k = 1, #HEART do
        local hx, hy = HEART[k][1], HEART[k][2]
        pts[k] = {
          p.x + (hx * cr - hy * sr) * s,
          p.y + (hx * sr + hy * cr) * s,
        }
      end
      ctx:polygon(pts, r, g, b, a8(ca, al / 255.0))
    end
  end
end
