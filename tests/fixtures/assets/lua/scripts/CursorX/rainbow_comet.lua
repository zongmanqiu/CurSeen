-- CursorX · Rainbow Comet
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：彗尾由逐段变色的粗线组成，头部是发光圆球；色相随时间推进。
-- 指针停在交互提示上时尾巴更宽更亮；点击炸出一圈彩虹火花。

name = "Rainbow Comet"
effect_name = "Rainbow Comet"
kind = "cursor"
description = "CursorX Rainbow Comet：全色域彗尾光标，点击迸出彩虹火花。"
hide_cursor = true

local sparks = {}
local trail = {}
local hue = 0
local phase = 0

function schema()
  return {
    { key = "trailLength", label = "尾巴长度", type = "int",   min = 10,  max = 80, step = 5,    default = 45 },
    { key = "hueSpeed",    label = "变色速度", type = "float", min = 0.5, max = 6,  step = 0.5,  default = 2.5 },
    { key = "maxWidth",    label = "最大宽度", type = "int",   min = 4,   max = 24, step = 1,    default = 13 },
    { key = "pointerAnim", label = "悬停增亮", type = "bool",  default = true },
    { key = "pointerGlowMult", label = "悬停辉光", type = "float", min = 1.0, max = 3.0, step = 0.1, default = 1.8 },
    { key = "clickAnim",   label = "点击火花", type = "bool",  default = true },
    { key = "clickSparkCount", label = "火花数量", type = "int", min = 5, max = 50, step = 1,   default = 22 },
  }
end

function init()
  sparks = {}
  trail = {}
  hue = 0
  phase = 0
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  local n = math.floor(params.clickSparkCount or 22)
  for _ = 1, n do
    local a = rng(0, math.pi * 2)
    local spd = rng(1.5, 5.0)
    sparks[#sparks + 1] = {
      x = x, y = y,
      vx = math.cos(a) * spd,
      vy = math.sin(a) * spd,
      hue = (hue + rng(-40, 40) + 360) % 360,
      life = 1.0,
      size = rng(2.5, 6.5),
    }
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local hover = cursor_hover and params.pointerAnim ~= false
  phase = phase + dt * (hover and 5.0 or 3.0)
  hue = (hue + (params.hueSpeed or 2.5) * f) % 360

  trail[#trail + 1] = { x = mouse_x or 0, y = mouse_y or 0, hue = hue }
  while #trail > math.floor(params.trailLength or 45) do table.remove(trail, 1) end

  for i = #sparks, 1, -1 do
    local p = sparks[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    p.life = p.life - 0.025 * f
    if p.life <= 0 then table.remove(sparks, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local hover = cursor_hover and params.pointerAnim ~= false
  local pulse = hover and (1.3 + math.sin(phase * 3.0) * 0.2) or 1.0
  local max_w = (params.maxWidth or 13) * pulse
  local glow_mul = hover and (params.pointerGlowMult or 1.8) or 1.0
  local n = #trail

  for i = 2, n do
    local a = trail[i - 1]
    local b = trail[i]
    local t = i / n
    local w = math.max(0.5, t * max_w)
    local r, g, bl = hsv_to_rgb(b.hue, 1.0, 0.62)
    local alpha = math.floor(255 * t * (hover and 0.95 or 0.88))
    ctx:line(a.x, a.y, b.x, b.y, w * 1.9, r, g, bl, math.floor(alpha * 0.18))
    ctx:line(a.x, a.y, b.x, b.y, w, r, g, bl, alpha)
  end

  if n > 0 then
    local h = trail[n]
    local head_r = hover and 11 or 7
    local hr, hg, hb = hsv_to_rgb(h.hue, 1.0, 0.82)
    ctx:glow(h.x, h.y, 22 * glow_mul * 0.35, hr, hg, hb, 180)
    ctx:circle(h.x, h.y, head_r, hr, hg, hb, 255)
    ctx:circle(h.x, h.y, head_r * 0.45, 255, 255, 255, 220)
  end

  for i = 1, #sparks do
    local p = sparks[i]
    local r, g, b = hsv_to_rgb(p.hue, 1.0, 0.75)
    local a = math.floor(255 * clamp(p.life, 0, 1))
    ctx:circle(p.x, p.y, math.max(0.4, p.size * p.life), r, g, b, a)
  end
end
