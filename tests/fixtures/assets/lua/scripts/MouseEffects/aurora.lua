-- MouseEffects · Aurora
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 光标上方的一带极光：多层水平波带叠在一起，颜色在青绿/青蓝/紫之间流动。
-- 上游是 4 层 GPU 噪声带；这里用 28 条竖直渐变线/层画出同样的"流动帘幕"。

name = "Aurora"
effect_name = "Aurora"
kind = "cursor"
description = "MouseEffects Aurora：光标上方的多层流动极光。"
hide_cursor = true

local SAMPLES = 56
local cx, cy = 0, 0
local phase = 0
local started = false

function schema()
  return {
    { key = "spread",    label = "横向宽度", type = "float", min = 300, max = 1400, step = 20, default = 800 },
    { key = "height",    label = "光帘高度", type = "float", min = 60,  max = 320,  step = 10, default = 170 },
    { key = "layers",    label = "层数",     type = "int",   min = 1,   max = 8,    step = 1,  default = 4 },
    { key = "speed",     label = "流速",     type = "float", min = 0.1, max = 3.0,  step = 0.1, default = 1.0 },
    { key = "intensity", label = "亮度",     type = "float", min = 0.2, max = 1.0,  step = 0.05, default = 0.65 },
    { key = "color1",    label = "主色",     type = "color", default = "#00FF80" },
    { key = "color2",    label = "辅色",     type = "color", default = "#00FFFF" },
    { key = "color3",    label = "点缀色",   type = "color", default = "#8B00FF" },
  }
end

function init()
  cx, cy = mouse_x or 0, mouse_y or 0
  phase = 0
  started = false
end

local function mix(a, b, t) return a * (1 - t) + b * t end

function step(dt, t, params)
  params = params or {}
  local mx, my = mouse_x or cx, mouse_y or cy
  if not started then
    cx, cy = mx, my
    started = true
  end
  local k = math.min(1.0, 0.16 * math.min(2.0, dt * 60.0))
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.speed or 1.0)
end

function draw(ctx, params)
  params = params or {}
  local spread = params.spread or 800
  local height = params.height or 170
  local layers = math.max(1, math.floor(params.layers or 4))
  local intensity = params.intensity or 0.65
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#00FF80")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#00FFFF")
  local c3r, c3g, c3b, c3a = hex_to_rgb(params.color3 or "#8B00FF")

  for layer = 1, layers do
    local u = (layer - 1) / math.max(1, layers - 1)
    local lr = math.floor(mix(mix(c1r, c2r, u * 0.5), c3r, u * 0.35))
    local lg = math.floor(mix(mix(c1g, c2g, u * 0.5), c3g, u * 0.35))
    local lb = math.floor(mix(mix(c1b, c2b, u * 0.5), c3b, u * 0.35))
    local base_y = cy - height * (0.10 + 0.22 * u)
    local amp = height * (0.20 + 0.10 * math.sin(phase * 0.7 + layer))
    local freq = 0.011 + 0.002 * layer
    local lay_a = mix(mix(c1a, c2a, u * 0.5), c3a, u * 0.35) / 255
    local alpha_mul = intensity * (1.0 - u * 0.35) * 0.72 * lay_a

    for i = 0, SAMPLES do
      local x = -spread * 0.5 + spread * i / SAMPLES
      local wave = math.sin(x * freq + phase * 1.3 + layer * 1.7) * amp
        + math.sin(x * freq * 2.3 - phase * 0.8 + layer) * amp * 0.35
      local edge = math.sin(math.pi * i / SAMPLES)
      local a = math.floor(255 * alpha_mul * edge * edge)
      if a > 2 then
        local top = base_y + wave - height * (0.30 + 0.06 * math.sin(phase + i * 0.3))
        local bot = base_y + wave + height * 0.18
        ctx:line(cx + x, top, cx + x, bot, 3.6, lr, lg, lb, a)
      end
    end

    -- 沿整条带宽铺三团柔光，避免只有中间一坨绿。
    for gx = -spread * 0.33, spread * 0.33, spread * 0.33 do
      ctx:glow(cx + gx, base_y + amp * 0.25, height * 0.30,
        lr, lg, lb, math.floor(34 * intensity * (1.0 - u * 0.4) * lay_a))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
