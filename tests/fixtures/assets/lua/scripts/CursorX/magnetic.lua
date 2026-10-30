-- CursorX · Magnetic
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类（桌面化重解释）：上游用 JS 把网页按钮吸向指针；桌面上没有可操作的
-- DOM，本实现把“磁力”落在指针周围的铁屑粒子场上——粒子缓慢环绕，指针附近被吸向
-- 圆点，离开后弹回轨道。指针停在交互提示上时磁力增强、圆点放大；点击弹一下并
-- 把铁屑向外推开。
--
-- 简化说明见包 README「已知简化」第 3 条。

name = "Magnetic"
effect_name = "Magnetic"
kind = "cursor"
description = "CursorX Magnetic：指针周围被吸住的铁屑场（桌面化重解释）。"
hide_cursor = true

local MAX_FILINGS = 56
local filings = {}
local cur_size = 12
local click_t = -1

function schema()
  return {
    { key = "color",       label = "光标颜色", type = "color", alpha = true, default = "#FFFFFF" },
    { key = "size",        label = "圆点大小", type = "int",   min = 4,    max = 28,  step = 1,    default = 12 },
    { key = "strength",    label = "磁场强度", type = "float", min = 0.003,max = 0.12,step = 0.003,default = 0.075 },
    { key = "pointerAnim", label = "悬停增强", type = "bool",  default = true },
    { key = "pointerScale",label = "悬停倍数", type = "float", min = 1.2,  max = 3.5, step = 0.1,  default = 2.0 },
    { key = "clickAnim",   label = "点击弹开", type = "bool",  default = true },
    { key = "clickScale",  label = "弹开幅度", type = "float", min = 1.2,  max = 4.5, step = 0.1,  default = 2.5 },
  }
end

function init()
  filings = {}
  cur_size = 12
  click_t = -1
  for i = 1, MAX_FILINGS do
    filings[i] = {
      angle = rng(0, math.pi * 2),
      radius = rng(26, 132),
      home = rng(26, 132),
      spin = rng(-0.008, 0.008),
      size = rng(1.2, 2.8),
      phase = rng(0, math.pi * 2),
    }
  end
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or 0, mouse_y or 0
  local hover = cursor_hover and params.pointerAnim ~= false
  local strength = params.strength or 0.075

  local target = (params.size or 12) * (hover and (params.pointerScale or 2.0) or 1.0)
  if click_t >= 0 and params.clickAnim ~= false then
    target = target * (1 + math.sin(click_t * math.pi) * ((params.clickScale or 2.5) - 1) * 0.6)
    click_t = click_t + dt * 4.5
    if click_t >= 1 then click_t = -1 end
  end
  cur_size = cur_size + (target - cur_size) * math.min(1.0, 0.18 * f)

  local pull = strength * (hover and 2.2 or 1.0)
  for i = 1, #filings do
    local p = filings[i]
    p.angle = p.angle + p.spin * f
    -- 内圈被吸住（半径随磁场收缩），外圈缓缓回位；两者都带一点正弦抖动。
    local target_r = p.home * (1.0 - pull * 2.6) + math.sin(p.phase + (elapsed or 0) * 1.4) * 4.0
    if click_t >= 0 then
      target_r = target_r + math.sin(click_t * math.pi) * 26.0
    end
    p.radius = p.radius + (target_r - p.radius) * math.min(1.0, 0.10 * f)
    p.phase = p.phase + dt * 0.9
  end
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, a = hex_to_rgb(params.color or "#FFFFFF")
  local mx, my = mouse_x or 0, mouse_y or 0
  local hover = cursor_hover and params.pointerAnim ~= false

  -- 三道很淡的磁场环，给“吸力范围”一个可读的形状。
  ctx:ring(mx, my, 46, 1.2, r, g, b, math.floor(a * 0.18))
  ctx:ring(mx, my, 82, 1.2, r, g, b, math.floor(a * 0.12))
  ctx:ring(mx, my, 118, 1.2, r, g, b, math.floor(a * 0.07))

  for i = 1, #filings do
    local p = filings[i]
    local x = mx + math.cos(p.angle) * p.radius
    local y = my + math.sin(p.angle) * p.radius * 0.82
    -- 短辐条指向磁极：没有它，铁屑只是一圈白点，读不出“磁力”。
    local ix = mx + math.cos(p.angle) * math.max(6, p.radius - 16)
    local iy = my + math.sin(p.angle) * math.max(6, p.radius - 16) * 0.82
    ctx:line(ix, iy, x, y, 0.8, r, g, b, math.floor(a * 0.16))
    local al = math.floor(a * (0.55 + 0.35 * math.sin(p.phase) * 0.5 + 0.175))
    ctx:circle(x, y, p.size, r, g, b, al)
  end

  -- 磁极：白点 + 黑色描边（任何背景都能找到指针）。
  ctx:circle(mx, my, cur_size * 0.5 + 1.5, 0, 0, 0, math.floor(a * 0.55))
  ctx:circle(mx, my, cur_size * 0.5, r, g, b, a)
  if hover then
    ctx:ring(mx, my, cur_size * 0.9, 1.4, r, g, b, math.floor(a * 0.5))
  end
end
