-- CursorFX · stars（click 版）
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：金色五角星从按下点向四周炸开，边飞边转、明暗闪烁后淡出。
-- 五角星由内外半径交替的十个顶点采样成 polygon（没有旋转原语）。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。
-- ★ 与同目录的 stars.lua（trail 版）是两个独立脚本，本文件专供点击宿主。

name = "stars"
effect_name = "stars"
kind = "click"
description = "CursorFX stars（click）：旋转闪烁的金色星爆。"

local MAX_STARS = 200
local PALETTE = { "#FFD700", "#FFF1A8", "#FFFFFF", "#FFB347" }

local stars = {}

function schema()
  return {
    { key = "color",   label = "单色",     type = "color", alpha = true, default = "#FFD700" },
    { key = "rainbow", label = "原版配色", type = "bool",  default = true },
    { key = "count",   label = "星数",     type = "int",   min = 4,  max = 40, step = 1,   default = 16 },
    { key = "size",    label = "星大小",   type = "float", min = 3,  max = 26, step = 0.5, default = 9 },
  }
end

function init()
  stars = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

function on_press(x, y, t, params)
  local n = math.floor(params.count or 16)
  if n < 1 then n = 1 elseif n > 40 then n = 40 end
  local size = params.size or 9
  local rainbow = params.rainbow ~= false
  local fixed = params.color or "#FFD700"

  for i = 1, n do
    local ang = (i / n) * 6.2831853 + rng(-0.2, 0.2)
    local sp = rng(180, 480)
    local life = rng(0.75, 1.25)
    stars[#stars + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp,
      rot = rng(0, 6.2831853),
      spin = rng(-12, 12),
      size = size * rng(0.66, 1.45),
      color = rainbow and PALETTE[rng_int(1, #PALETTE)] or fixed,
      life = life,
      max = life,
    }
  end

  while #stars > MAX_STARS do
    table.remove(stars, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local damp = (0.93) ^ (ds * 60.0)
  local n = #stars
  local w = 0
  for i = 1, n do
    local p = stars[i]
    p.vx = p.vx * damp
    p.vy = p.vy * damp
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.rot = p.rot + p.spin * ds
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      stars[w] = p
    end
  end
  for i = #stars, w + 1, -1 do
    stars[i] = nil
  end
end

function draw(ctx, params)
  if #stars == 0 then return end

  local now = elapsed or 0
  for i = 1, #stars do
    local p = stars[i]
    local t = p.life / p.max
    local tw = 0.7 + 0.3 * math.sin(now * 14.0 + p.rot * 3.0)
    local al = a8(255, math.min(1.0, t * 2.0) * tw)
    if al > 0 then
      local r, g, b, ca = hex_to_rgb(p.color)
      local radius = p.size * (0.4 + t * 0.6)
      local pts = {}
      for k = 0, 9 do
        local rad = radius
        if k % 2 == 1 then rad = radius * 0.45 end
        local ang = (k / 10.0) * 6.2831853 + p.rot - 1.5707963
        pts[k + 1] = {
          p.x + math.cos(ang) * rad,
          p.y + math.sin(ang) * rad,
        }
      end
      ctx:polygon(pts, r, g, b, a8(ca, al / 255.0))
    end
  end
end
