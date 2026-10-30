-- CursorX · Gravity Pull
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类（桌面化重解释）：上游把网页里的元素按重力公式拉向指针；桌面上没有
-- 可操作的 DOM，本实现把“引力”落在环绕指针的碎屑上——碎屑按 strength/d² 加速坠向
-- 指针，靠近后回弹到各自的轨道半径。指针停在交互提示上时圆环呼吸，点击闪一圈。
--
-- 简化说明见包 README「已知简化」第 3 条。

name = "Gravity Pull"
effect_name = "Gravity Pull"
kind = "cursor"
description = "CursorX Gravity Pull：碎屑坠向指针的引力场（桌面化重解释）。"
hide_cursor = true

local MAX_DEBRIS = 34
local debris = {}
local ring_w = 36
local pulse_phase = 0
local flash_t = -1

function schema()
  return {
    { key = "color",      label = "引力颜色", type = "color", alpha = true, default = "#5CF4FC" },
    { key = "strength",   label = "引力常数", type = "int",   min = 5000, max = 50000, step = 1000, default = 25000 },
    { key = "radius",     label = "作用半径", type = "int",   min = 80,   max = 400,  step = 20,    default = 220 },
    { key = "damping",    label = "阻尼",     type = "float", min = 0.7,  max = 0.95, step = 0.01,  default = 0.84 },
    { key = "pointerAnim",label = "悬停呼吸", type = "bool",  default = true },
    { key = "clickAnim",  label = "点击闪光", type = "bool",  default = true },
  }
end

function init()
  debris = {}
  ring_w = 36
  pulse_phase = 0
  flash_t = -1
  for i = 1, MAX_DEBRIS do
    local a = rng(0, math.pi * 2)
    local d = rng(46, 190)
    debris[i] = {
      angle = a,
      radius = d,
      home = d,
      vr = 0,
      spin = rng(-0.012, 0.012),
      size = rng(1.3, 3.2),
    }
  end
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then flash_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or 0, mouse_y or 0
  local hover = cursor_hover and params.pointerAnim ~= false
  local damping = math.pow(params.damping or 0.84, f)
  local strength = params.strength or 25000
  local reach = params.radius or 220

  -- 圆环大小呼吸
  if hover then
    pulse_phase = pulse_phase + dt * 3.0
    ring_w = 36 + math.sin(pulse_phase) * 10
  else
    pulse_phase = 0
    ring_w = ring_w + (36 - ring_w) * math.min(1.0, 0.12 * f)
  end
  if flash_t >= 0 then
    flash_t = flash_t + dt * 4.2
    if flash_t >= 1 then flash_t = -1 end
  end

  for i = 1, #debris do
    local d = debris[i]
    d.angle = d.angle + d.spin * f
    local x = mx + math.cos(d.angle) * d.radius
    local y = my + math.sin(d.angle) * d.radius
    local dist = math.max(1.0, math.sqrt((x - mx) ^ 2 + (y - my) ^ 2))
    local acc = 0.0
    if dist < reach then
      acc = math.min(strength / (dist * dist), 200) * 0.016
    end
    d.vr = (d.vr - acc * f) * damping + (d.home - d.radius) * 0.006 * f
    d.radius = math.max(10, d.radius + d.vr * f)
  end
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, a = hex_to_rgb(params.color or "#5CF4FC")
  local mx, my = mouse_x or 0, mouse_y or 0
  local hover = cursor_hover and params.pointerAnim ~= false
  local rw = ring_w
  if flash_t >= 0 and params.clickAnim ~= false then
    rw = rw * (1 + math.sin(flash_t * math.pi * 0.7) * 0.9)
  end

  ctx:ring(mx, my, rw * 0.5 + 2.0, 4.0, r, g, b, math.floor(a * 0.14))
  ctx:ring(mx, my, rw * 0.5, 2.0, r, g, b, a)

  for i = 1, #debris do
    local d = debris[i]
    local x = mx + math.cos(d.angle) * d.radius
    local y = my + math.sin(d.angle) * d.radius
    ctx:circle(x, y, d.size, r, g, b, math.floor(a * 0.9))
    ctx:plus_circle(x, y, d.size * 2.4, r, g, b, math.floor(a * 0.16))
  end
  if hover then
    ctx:ring(mx, my, (params.radius or 220) * 0.5, 1.0, r, g, b, math.floor(a * 0.10))
  end

  local flash = 0
  if flash_t >= 0 and params.clickAnim ~= false then
    flash = math.sin(flash_t * math.pi)
    ctx:ring(mx, my, rw * 0.5 * (1.0 + flash * 0.7), 2.0, r, g, b, math.floor(160 * flash))
  end
  ctx:circle(mx, my, 2.0, 0, 0, 0, 120)
  ctx:circle(mx, my, 1.2, 255, 255, 255, 230)
end
