-- CursorFX · confetti
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：纸屑礼炮。彩纸片从按下点向上扇形喷出，受重力与空气阻尼下落，同时
-- 自转并周期性翻面。翻面用 y 轴压缩 + 旋转四边形近似（CurSeen 没有 canvas 变换）。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。

name = "confetti"
effect_name = "confetti"
kind = "click"
description = "CursorFX confetti：向上喷出、翻滚飘落的彩纸礼炮。"

local MAX_CONFETTI = 240
local PALETTE = { "#FF595E", "#FFCA3A", "#8AC926", "#1982C4", "#6A4C93", "#FFFFFF" }

local confetti = {}

function schema()
  return {
    { key = "color",   label = "单色",     type = "color", alpha = true, default = "#FF595E" },
    { key = "rainbow", label = "原版配色", type = "bool",  default = true },
    { key = "count",   label = "纸片数",   type = "int",   min = 10, max = 120, step = 1,   default = 60 },
    { key = "spread",  label = "散布角",   type = "float", min = 0.2, max = 1.57, step = 0.05, default = 1.1 },
    { key = "size",    label = "纸片大小", type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
    { key = "life",    label = "寿命倍率", type = "float", min = 0.5, max = 1.5, step = 0.05, default = 1.0 },
  }
end

function init()
  confetti = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

function on_press(x, y, t, params)
  local n = math.floor(params.count or 60)
  if n < 1 then n = 1 elseif n > 120 then n = 120 end
  local spread = params.spread or 1.1
  local size = params.size or 1.0
  local life_k = params.life or 1.0
  local rainbow = params.rainbow ~= false
  local fixed = params.color or "#FF595E"

  for i = 1, n do
    local ang = -1.5707963 + rng(-spread, spread)
    local sp = rng(240, 780)
    local life = rng(1.5, 2.5) * life_k
    confetti[#confetti + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp,
      w = rng(5, 10) * size,
      h = rng(3, 6) * size,
      rot = rng(0, 6.2831853),
      spin = rng(-18, 18),
      tilt = rng(0, 6.2831853),
      ts = rng(6, 18),
      color = rainbow and PALETTE[rng_int(1, #PALETTE)] or fixed,
      life = life,
      max = life,
    }
  end

  while #confetti > MAX_CONFETTI do
    table.remove(confetti, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local damp = (0.95) ^ (ds * 60.0)
  local n = #confetti
  local w = 0
  for i = 1, n do
    local p = confetti[i]
    p.vy = p.vy + 576.0 * ds
    p.vx = p.vx * damp
    p.vy = p.vy * damp
    p.x = p.x + (p.vx + math.sin(p.tilt) * 36.0) * ds
    p.y = p.y + p.vy * ds
    p.rot = p.rot + p.spin * ds
    p.tilt = p.tilt + p.ts * ds
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      confetti[w] = p
    end
  end
  for i = #confetti, w + 1, -1 do
    confetti[i] = nil
  end
end

local function paper(ctx, p, ca)
  local t = p.life / p.max
  local al = a8(ca, math.min(1.0, t * 4.0))
  if al <= 0 or p.w <= 0 or p.h <= 0 then return end

  local r, g, b = hex_to_rgb(p.color)
  local sy = math.abs(math.cos(p.tilt))
  if sy < 0.15 then sy = 0.15 end
  local hw = p.w * 0.5
  local hh = p.h * 0.5 * sy
  local cr, sr = math.cos(p.rot), math.sin(p.rot)

  local pts = {
    { p.x - hw * cr + hh * sr, p.y - hw * sr - hh * cr },
    { p.x + hw * cr + hh * sr, p.y + hw * sr - hh * cr },
    { p.x + hw * cr - hh * sr, p.y + hw * sr + hh * cr },
    { p.x - hw * cr - hh * sr, p.y - hw * sr + hh * cr },
  }
  ctx:polygon(pts, r, g, b, al)
end

function draw(ctx, params)
  if #confetti == 0 then return end
  local _, _, _, ca = hex_to_rgb(params.color or "#FF595E")
  if ca <= 0 then return end
  for i = 1, #confetti do
    paper(ctx, confetti[i], ca)
  end
end
