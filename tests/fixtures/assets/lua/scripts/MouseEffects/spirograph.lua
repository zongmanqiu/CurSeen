-- MouseEffects · Spirograph
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的万花尺图案：按花瓣数生成内摆线，整圈缓慢旋转，颜色沿曲线渐变。
-- 上游把图案画在鼠标移动轨迹上；这里做成跟随光标的完整图案（README 注明简化）。

name = "Spirograph"
effect_name = "Spirograph"
kind = "cursor"
description = "MouseEffects Spirograph：跟随光标的旋转万花尺图案。"
hide_cursor = true

local SAMPLES = 280
local cx, cy = 0, 0
local phase = 0

function schema()
  return {
    { key = "petals",  label = "花瓣数",   type = "int",   min = 3,   max = 24,  step = 1,   default = 12 },
    { key = "radius",  label = "图案半径", type = "float", min = 50,  max = 300, step = 10,  default = 120 },
    { key = "pen",     label = "笔尖距离", type = "float", min = 20,  max = 200, step = 5,   default = 80 },
    { key = "width",   label = "线宽",     type = "float", min = 0.5, max = 5,   step = 0.5, default = 2 },
    { key = "spin",    label = "旋转速度", type = "float", min = 0,   max = 2,   step = 0.1, default = 1.0 },
    { key = "glow",    label = "辉光强度", type = "float", min = 0,   max = 3,   step = 0.1, default = 1.5 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.1, max = 1,   step = 0.05, default = 0.8 },
    { key = "color1",  label = "起始颜色", type = "color", default = "#FF0080" },
    { key = "color2",  label = "结束颜色", type = "color", default = "#0080FF" },
  }
end

function init()
  cx, cy = mouse_x or 0, mouse_y or 0
  phase = 0
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.16 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.spin or 1.0) * 1.2
end

function draw(ctx, params)
  params = params or {}
  local petals = math.max(3, math.floor(params.petals or 12))
  local R = params.radius or 120
  local d = params.pen or 80
  local width = params.width or 2
  local glow = params.glow or 1.5
  local alpha = math.floor(255 * (params.opacity or 0.8))
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#FF0080")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#0080FF")

  local r = R / petals
  local prev = nil
  for i = 0, SAMPLES do
    local a = i / SAMPLES * math.pi * 2 + phase
    local k = (R - r) / r
    local x = cx + (R - r) * math.cos(a) + d * math.cos(k * a)
    local y = cy + (R - r) * math.sin(a) + d * math.sin(k * a)
    if prev then
      local u = i / SAMPLES
      ctx:line(prev[1], prev[2], x, y, width,
        math.floor(c1r + (c2r - c1r) * u),
        math.floor(c1g + (c2g - c1g) * u),
        math.floor(c1b + (c2b - c1b) * u), math.floor(alpha * (c1a + (c2a - c1a) * u) / 255))
    end
    prev = { x, y }
    if i % 20 == 0 then
      ctx:plus_circle(x, y, 1.6, 255, 255, 255, math.floor(alpha * 0.7 * (c1a + c2a) * 0.5 / 255))
    end
  end

  if glow > 0.01 then
    ctx:glow(cx, cy, R * 0.65, c1r, c1g, c1b, math.floor(35 * math.min(1.0, glow) * c1a / 255))
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
