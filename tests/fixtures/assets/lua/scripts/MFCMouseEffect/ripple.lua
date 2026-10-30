-- MFCMouseEffect · Ripple
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击水波纹：鼠标抬起时在点击点生出一圈主环 + 3 层外发光环；三次方缓出扩散，
-- 透明度随进度线性淡出。按住 ≥260ms 视为长按，不触发本次点击（与上游一致）。

name = "Ripple"
effect_name = "Ripple"
kind = "click"
description = "MFCMouseEffect Ripple：点击点扩散的水波纹。"

local HOLD_SUPPRESS_MS = 260
local MAX_PULSES = 32
local pulses = {}
local press_ms = 0
local press_button = "left"

function schema()
  return {
    { key = "color",        label = "左键颜色", type = "color", alpha = true, default = "#0288D1" },
    { key = "right_color",  label = "右键颜色", type = "color", alpha = true, default = "#FF6F00" },
    { key = "middle_color", label = "中键颜色", type = "color", alpha = true, default = "#0B8043" },
    { key = "start_radius", label = "起始半径", type = "float", min = 0,  max = 40, step = 1,    default = 8 },
    { key = "end_radius",   label = "结束半径", type = "float", min = 10, max = 160, step = 2,   default = 44 },
    { key = "width",        label = "环宽",     type = "float", min = 0.5,max = 8,  step = 0.1,  default = 2.2 },
    { key = "duration",     label = "时长(ms)", type = "int",   min = 80, max = 1200, step = 10, default = 260 },
  }
end

local function color_for(button, params)
  if button == "right" then return params.right_color or "#FF6F00" end
  if button == "middle" then return params.middle_color or "#0B8043" end
  return params.color or "#0288D1"
end

function init()
  pulses = {}
  press_ms = 0
  press_button = "left"
end

function on_mouse_down(button, x, y, params)
  press_ms = wall_ms or 0
  press_button = button or "left"
end

function on_mouse_up(button, x, y, params)
  local held = (wall_ms or 0) - press_ms
  if held >= HOLD_SUPPRESS_MS then return end
  local btn = button or press_button
  local r, g, b, a = hex_to_rgb(color_for(btn, params or {}))
  pulses[#pulses + 1] = {
    x = x, y = y,
    start = wall_ms or 0,
    duration = math.max(80, params.duration or 260),
    start_r = params.start_radius or 8,
    end_r = params.end_radius or 44,
    width = params.width or 2.2,
    r = r, g = g, b = b, a = a,
  }
  while #pulses > MAX_PULSES do table.remove(pulses, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  for i = #pulses, 1, -1 do
    if now - pulses[i].start >= pulses[i].duration then table.remove(pulses, i) end
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  for i = 1, #pulses do
    local p = pulses[i]
    local t = clamp((now - p.start) / p.duration, 0, 1)
    local eased = 1 - (1 - t) * (1 - t) * (1 - t)
    local fade = 1 - t
    local radius = p.start_r + (p.end_r - p.start_r) * eased
    if radius < 0.5 then radius = 0.5 end
    local ring_w = math.max(p.width * 1.34, math.min(radius * 0.18,
      math.max(p.width + 1.25, 2.6)))
    local a = p.a * fade
    -- 三层外发光：越靠外越淡，模拟 GDI+ 的多层 Pen。
    -- ★ 辉光底 alpha：上游 click 辉光色自带低 alpha 底（0x52×0.62≈51，即满
    --   alpha 的 ≈0.2 倍，ClickEffectCommandAdapter），此前用主色满 alpha，
    --   辉光亮了约 5 倍。
    local glow_base = a * 0.2
    for k = 0, 2 do
      local w = ring_w + 5.0 + k * 2.8
      ctx:ring(p.x, p.y, radius, w, p.r, p.g, p.b,
        math.floor(glow_base * (0.15 - k * 0.03) * 5))
    end
    ctx:ring(p.x, p.y, radius, math.max(p.width * 0.94, ring_w * 0.18),
      p.r, p.g, p.b, math.floor(a))
  end
end
