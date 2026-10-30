-- CursorX · Elastic Ring
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：弹簧环拖在指针后面，速度越快沿运动方向拉得越长；指针停在交互提示上
-- 时环会张开包住目标；点击时整环脉冲。白点始终钉在真实指针位置。

name = "Elastic Ring"
effect_name = "Elastic Ring"
kind = "cursor"
description = "CursorX Elastic Ring：带弹簧物理的拉伸圆环光标。"
hide_cursor = true

local TAU = math.pi * 2

local pos = { x = 0, y = 0, vx = 0, vy = 0 }
local click_t = -1

function schema()
  return {
    { key = "color",      label = "圆环颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "size",       label = "圆环大小", type = "int",   min = 16,  max = 80,  step = 2,    default = 38 },
    { key = "stiffness",  label = "弹簧刚度", type = "float", min = 0.04,max = 0.4, step = 0.01, default = 0.13 },
    { key = "damping",    label = "阻尼",     type = "float", min = 0.5, max = 0.95,step = 0.02, default = 0.76 },
    { key = "pointerAnim",label = "悬停张开", type = "bool",  default = true },
    { key = "clickAnim",  label = "点击脉冲", type = "bool",  default = true },
    { key = "clickScale", label = "点击倍数", type = "float", min = 1.2, max = 3.5, step = 0.1,  default = 1.8 },
  }
end

function init()
  pos = { x = mouse_x or 0, y = mouse_y or 0, vx = 0, vy = 0 }
  click_t = -1
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then
    click_t = 0
  end
end

local function ring_polyline(ctx, cx, cy, rx, ry, angle, width, r, g, b, a)
  local ca, sa = math.cos(angle), math.sin(angle)
  local n = 40
  local px, py
  for i = 0, n do
    local t = i / n * TAU
    local ex, ey = math.cos(t) * rx, math.sin(t) * ry
    local x = cx + ex * ca - ey * sa
    local y = cy + ex * sa + ey * ca
    if i > 0 then
      ctx:line(px, py, x, y, width, r, g, b, a)
    end
    px, py = x, y
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local stiffness = params.stiffness or 0.13
  local damping = params.damping or 0.76
  local mx, my = mouse_x or pos.x, mouse_y or pos.y

  pos.vx = (pos.vx + (mx - pos.x) * stiffness * f) * math.pow(damping, f)
  pos.vy = (pos.vy + (my - pos.y) * stiffness * f) * math.pow(damping, f)
  pos.x = pos.x + pos.vx * f
  pos.y = pos.y + pos.vy * f

  if click_t >= 0 then
    click_t = click_t + dt * 3.6
    if click_t >= 1 then click_t = -1 end
  end
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#7C5CFC")
  local base = params.size or 38
  local a = ca

  local speed = math.sqrt(pos.vx * pos.vx + pos.vy * pos.vy)
  local stretch = math.min(speed * 0.07, 0.55)
  local angle = math.atan2(pos.vy, pos.vx)

  local scale = 1.0
  if click_t >= 0 and params.clickAnim ~= false then
    local cs = params.clickScale or 1.8
    scale = 1.0 + math.sin(click_t * math.pi) * (cs - 1.0)
  end

  local hover = cursor_hover and params.pointerAnim ~= false
  local half = base * 0.5 * scale
  if hover then
    half = base * 0.85
    stretch = 0
  end

  if stretch < 0.04 then
    ctx:ring(pos.x, pos.y, half + 1.5, 1.5, r, g, b, math.floor(a * 0.22))
    ctx:ring(pos.x, pos.y, half, 2.0, r, g, b, a)
  else
    local rx = half * (1.0 + stretch)
    local ry = half * (1.0 - stretch * 0.5)
    ring_polyline(ctx, pos.x, pos.y, rx + 1.2, ry + 1.2, angle, 1.4, r, g, b, math.floor(a * 0.25))
    ring_polyline(ctx, pos.x, pos.y, rx, ry, angle, 2.0, r, g, b, a)
  end

  local mx, my = mouse_x or pos.x, mouse_y or pos.y
  ctx:circle(mx, my, 3.6, 0, 0, 0, 150)
  ctx:circle(mx, my, 2.6, 255, 255, 255, 255)
end
