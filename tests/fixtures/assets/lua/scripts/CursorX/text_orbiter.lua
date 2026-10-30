-- CursorX · Text Orbiter
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：一串字符绕指针公转，中心是一颗发光小点；指针停在交互提示上时
-- 轨道半径与转速一起增大，点击时先扩张再收回。
-- 简化：渲染层没有旋转文字，字符保持正立（上游是沿切线旋转的）。

name = "Text Orbiter"
effect_name = "Text Orbiter"
kind = "cursor"
description = "CursorX Text Orbiter：绕指针公转的文字环。"
hide_cursor = true

local angle = 0
local radius = 42
local speed_now = 0.02
local click_t = -1
local glyphs = {}
local glyph_src = ""

-- 没有 utf8 库：按首字节判断码点长度，避免把 ✦ 这类多字节字符劈开。
local function split_utf8(s)
  local out = {}
  local i = 1
  local n = #s
  while i <= n and #out < 48 do
    local c = s:byte(i)
    local len = 1
    if c >= 0xF0 then len = 4
    elseif c >= 0xE0 then len = 3
    elseif c >= 0xC0 then len = 2 end
    out[#out + 1] = s:sub(i, math.min(n, i + len - 1))
    i = i + len
  end
  return out
end

function schema()
  return {
    { key = "text",             label = "环绕文字", type = "str",   default = "✦ CURSORX ✦ DEV ✦ FELIXAU" },
    { key = "color",            label = "文字颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "radius",           label = "轨道半径", type = "int",   min = 20,  max = 80,  step = 2,    default = 42 },
    { key = "speed",            label = "旋转速度", type = "float", min = 0.005,max = 0.06,step = 0.005,default = 0.02 },
    { key = "pointerAnim",      label = "悬停加速", type = "bool",  default = true },
    { key = "pointerRadiusBoost", label = "半径增量", type = "int", min = 5, max = 45, step = 1,    default = 20 },
    { key = "pointerSpeedBoost",  label = "速度增量", type = "float", min = 0.01, max = 0.1, step = 0.005, default = 0.04 },
    { key = "clickAnim",        label = "点击扩张", type = "bool",  default = true },
  }
end

function init()
  angle = 0
  radius = 42
  speed_now = 0.02
  click_t = -1
  glyphs = {}
  glyph_src = ""
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local text = params.text or "✦ CURSORX ✦ DEV ✦ FELIXAU"
  if text ~= glyph_src then
    glyph_src = text
    glyphs = split_utf8(text)
  end

  local base_r = params.radius or 42
  local base_s = params.speed or 0.02
  local target_r, target_s = base_r, base_s
  local hover = cursor_hover and params.pointerAnim ~= false
  if hover then
    target_r = base_r + (params.pointerRadiusBoost or 20)
    target_s = base_s + (params.pointerSpeedBoost or 0.04)
  elseif click_t >= 0 and params.clickAnim ~= false then
    local k = math.sin(click_t * math.pi)
    target_r = base_r + k * (params.pointerRadiusBoost or 20)
    target_s = base_s + k * (params.pointerSpeedBoost or 0.04)
    click_t = click_t + dt * 2.1
    if click_t >= 1 then click_t = -1 end
  end

  local k = math.min(1.0, 0.08 * f)
  radius = radius + (target_r - radius) * k
  speed_now = speed_now + (target_s - speed_now) * k
  angle = angle + speed_now * f
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, a = hex_to_rgb(params.color or "#7C5CFC")
  local mx, my = mouse_x or 0, mouse_y or 0
  local n = #glyphs

  if n > 0 then
    local step_a = math.pi * 2 / n
    for i = 1, n do
      local ca = angle + (i - 1) * step_a
      local x = mx + math.cos(ca) * radius
      local y = my + math.sin(ca) * radius
      ctx:plus_circle(x, y, 5.0, r, g, b, math.floor(a * 0.10))
      ctx:text_centered(x, y, glyphs[i], 10, r, g, b, a)
    end
  end

  ctx:glow(mx, my, 8.0, r, g, b, math.floor(a * 0.7))
  ctx:circle(mx, my, 3.0, r, g, b, a)
  ctx:circle(mx, my, 1.2, 255, 255, 255, 220)
end
