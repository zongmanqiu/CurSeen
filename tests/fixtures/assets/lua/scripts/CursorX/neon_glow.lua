-- CursorX · Neon Glow
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：霓虹光点常驻呼吸；指针停在交互提示上时放大脉冲；点击按原版色环
-- 依次换色（首次点击前用参数里的颜色）。

name = "Neon Glow"
effect_name = "Neon Glow"
kind = "cursor"
description = "CursorX Neon Glow：呼吸的霓虹光点，点击依次换色。"
hide_cursor = true

local COLORS = {
  { 124, 92, 252 }, { 92, 244, 252 }, { 252, 92, 184 },
  { 92, 252, 168 }, { 252, 168, 92 },
}

local phase = 0
local cur_size = 10
local cx, cy = 0, 0
local color_idx = 0
local color_src = ""

function schema()
  return {
    { key = "color",       label = "霓虹颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "size",        label = "光点大小", type = "int",   min = 6,   max = 30,  step = 1,    default = 10 },
    { key = "pulseSpeed",  label = "呼吸速度", type = "float", min = 0.3, max = 3,   step = 0.1,  default = 0.5 },
    { key = "glowRadius",  label = "辉光半径", type = "int",   min = 5,   max = 50,  step = 1,    default = 5 },
    { key = "opacity",     label = "不透明度", type = "float", min = 0.2, max = 1,   step = 0.05, default = 1.0 },
    { key = "pointerAnim", label = "悬停脉冲", type = "bool",  default = true },
    { key = "pointerScale",label = "悬停倍数", type = "float", min = 1.3, max = 4,   step = 0.1,  default = 3.0 },
    { key = "clickAnim",   label = "点击换色", type = "bool",  default = true },
  }
end

function init()
  phase = 0
  cur_size = 10
  color_idx = 0
  color_src = ""
  cx, cy = mouse_x or 0, mouse_y or 0
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  -- 0 = 参数里的颜色；1..#COLORS = 预设色。循环必须回到 0，否则选过的颜色
  -- 在第一次点击后就永远轮不到了。
  color_idx = (color_idx + 1) % (#COLORS + 1)
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  phase = phase + dt / math.max(0.1, params.pulseSpeed or 0.5)

  local base = params.size or 10
  local pulse_scale = params.pointerScale or 3.0
  local target
  if cursor_hover and params.pointerAnim ~= false then
    target = base * (1.0 + (math.sin(phase * math.pi * 2.0) * 0.5 + 0.5) * (pulse_scale - 1.0))
  else
    target = base * (1.0 + math.sin(phase * math.pi * 2.0) * 0.28)
  end
  cur_size = cur_size + (target - cur_size) * math.min(1.0, 0.12 * f)

  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.16 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
end

function draw(ctx, params)
  params = params or {}
  -- 改颜色参数要立刻生效：不重置的话，用户点过几次后再调色，要再点回去才看得到。
  if params.color ~= color_src then
    color_src = params.color
    color_idx = 0
  end
  local col = COLORS[color_idx]
  local r, g, b, ca
  if col and color_idx > 0 then
    r, g, b, ca = col[1], col[2], col[3], 255
  else
    r, g, b, ca = hex_to_rgb(params.color or "#7C5CFC")
  end
  local alpha = (params.opacity or 1.0) * (cursor_hover and 0.65 or 1.0) * ca / 255
  local a = math.floor(255 * clamp(alpha, 0, 1))
  local glow = (params.glowRadius or 5) + cur_size * 0.55

  ctx:glow(cx, cy, glow, r, g, b, math.floor(a * 0.85))
  ctx:circle(cx, cy, math.max(1.0, cur_size * 0.5), r, g, b, a)
  -- 中心高光：浅色背景上也能确认指针位置。
  ctx:circle(cx - cur_size * 0.12, cy - cur_size * 0.12, math.max(0.8, cur_size * 0.16),
    255, 255, 255, math.floor(a * 0.75))
end
