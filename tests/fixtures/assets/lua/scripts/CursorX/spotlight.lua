-- CursorX · Spotlight
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：整屏压暗，只在指针周围留一圈柔边光孔；指针停在交互提示上时光孔
-- 呼吸放大，点击脉冲一次。暗罩必须留在 base 层：clear_circle 是破坏性擦除，
-- 垫底才不会清掉同帧的拖尾/点击特效。

name = "Spotlight"
effect_name = "Spotlight"
kind = "cursor"
description = "CursorX Spotlight：整屏压暗，指针处留柔边光孔。"
hide_cursor = true

local px, py = 0, 0
local radius = 160
local phase = 0
local click_t = -1
local started = false

function schema()
  return {
    { key = "radius",      label = "光圈半径", type = "int",   min = 60,  max = 360, step = 10,   default = 160 },
    { key = "darkness",    label = "暗罩浓度", type = "float", min = 0.5, max = 0.98,step = 0.02, default = 0.90 },
    { key = "pointerAnim", label = "悬停呼吸", type = "bool",  default = true },
    { key = "pointerPulse",label = "呼吸幅度", type = "int",   min = 10,  max = 90,  step = 5,    default = 40 },
    { key = "clickAnim",   label = "点击脉冲", type = "bool",  default = true },
    { key = "clickPulse",  label = "脉冲幅度", type = "int",   min = 20,  max = 130, step = 10,   default = 70 },
  }
end

function init()
  px, py = mouse_x or 0, mouse_y or 0
  radius = 160
  phase = 0
  click_t = -1
  started = true
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or px, mouse_y or py

  local k = math.min(1.0, 0.35 * f)
  px = px + (mx - px) * k
  py = py + (my - py) * k

  local base = params.radius or 160
  local target = base
  if click_t >= 0 and params.clickAnim ~= false then
    local pulse = math.sin(click_t * math.pi)
    target = base + pulse * (params.clickPulse or 70)
    click_t = click_t + dt * 2.4
    if click_t >= 1 then click_t = -1 end
  elseif cursor_hover and params.pointerAnim ~= false then
    phase = phase + dt * 2.1
    target = base + math.sin(phase) * (params.pointerPulse or 40)
  else
    phase = 0
  end
  radius = radius + (target - radius) * math.min(1.0, 0.1 * f)
end

function draw(ctx, params)
  params = params or {}
  local darkness = clamp(params.darkness or 0.90, 0, 1)
  local r = math.max(radius, 2.0)

  ctx:layer("base")
  ctx:rect(screen_x or 0, screen_y or 0, screen_w or 0, screen_h or 0, 0, 0, 0,
    math.floor(255 * darkness))
  ctx:clear_circle(px, py, r)
  -- 柔边半径比清洞略大：让渐变的抗锯齿边缘落在均匀暗罩里，避免与 clear_circle
  -- 的边缘对不齐时出现一圈 1px 接缝。
  ctx:radial_stops(px, py, r + 2.0, {
    { offset = 0.0, r = 0, g = 0, b = 0, a = 0 },
    { offset = 0.82, r = 0, g = 0, b = 0, a = math.floor(40 * darkness) },
    { offset = 1.0, r = 0, g = 0, b = 0, a = math.floor(255 * darkness) },
  })

  ctx:layer("normal")
  local mx, my = mouse_x or px, mouse_y or py
  local glow = 10 + math.sin((elapsed or 0) * 3.0) * 1.5
  ctx:glow(mx, my, glow, 255, 200, 70, 190)
  ctx:circle(mx, my, 3.5, 255, 204, 68, 255)
end
