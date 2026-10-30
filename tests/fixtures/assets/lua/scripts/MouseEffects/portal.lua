-- MouseEffects · Portal
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的传送门：深色内芯、数层反向旋转的环、四条螺旋臂和沿边粒子。
-- 上游用 GPU 螺旋噪声；这里用圆环 + 折线螺旋画出同样的旋涡层次。

name = "Portal"
effect_name = "Portal"
kind = "cursor"
description = "MouseEffects Portal：旋转的传送门与螺旋臂。"
hide_cursor = true

local phase = 0
local cx, cy = 0, 0

function schema()
  return {
    { key = "radius",    label = "传送门半径", type = "float", min = 60,  max = 220, step = 5,   default = 150 },
    { key = "arms",      label = "螺旋臂数",   type = "int",   min = 2,   max = 8,   step = 1,   default = 4 },
    { key = "spin",      label = "旋转速度",   type = "float", min = 0,   max = 3,   step = 0.1, default = 1.0 },
    { key = "tightness", label = "螺旋紧度",   type = "float", min = 0.1, max = 3,   step = 0.1, default = 1.0 },
    { key = "glow",      label = "辉光强度",   type = "float", min = 0,   max = 3,   step = 0.1, default = 1.2 },
    { key = "darkness",  label = "内芯暗度",   type = "float", min = 0,   max = 1,   step = 0.05, default = 0.55 },
    { key = "color1",    label = "主色",       type = "color", default = "#66B3FF" },
    { key = "color2",    label = "旋臂色",     type = "color", default = "#CCE6FF" },
  }
end

function init()
  phase = 0
  cx, cy = mouse_x or 0, mouse_y or 0
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.14 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.spin or 1.0)
end

function draw(ctx, params)
  params = params or {}
  local R = params.radius or 150
  local arms = math.max(2, math.floor(params.arms or 4))
  local tight = params.tightness or 1.0
  local glow = params.glow or 1.2
  local dark = params.darkness or 0.55
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#66B3FF")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#CCE6FF")
  local am1, am2 = c1a / 255, c2a / 255

  -- 深色内芯：中心黑、边缘透明的径向渐变。
  ctx:radial_stops(cx, cy, R * 0.82, {
    { offset = 0.0, r = 4, g = 6, b = 12, a = math.floor(235 * dark) },
    { offset = 0.55, r = 8, g = 12, b = 26, a = math.floor(150 * dark) },
    { offset = 1.0, r = c1r, g = c1g, b = c1b, a = 0 },
  })

  -- 数层旋转环：奇偶层反向，越靠外越淡。
  local rings = 6
  for i = 1, rings do
    local u = i / rings
    local rr = R * (0.18 + 0.78 * u)
    local dir = (i % 2 == 0) and 1 or -1
    local wob = 1 + 0.035 * math.sin(phase * 2.0 * dir + i)
    local a = math.floor(180 * (0.35 + 0.65 * u) * (1 - u * 0.25) * (am1 * 0.7 + am2 * 0.3))
    ctx:ring(cx, cy, rr * wob, 1.2 + u * 1.6,
      math.floor(c1r * 0.7 + c2r * 0.3),
      math.floor(c1g * 0.7 + c2g * 0.3),
      math.floor(c1b * 0.7 + c2b * 0.3), a)
  end

  -- 螺旋臂：从内圈绕到外圈，角度随半径推进。
  local segs = 22
  for arm = 1, arms do
    local base = arm * math.pi * 2 / arms + phase * 0.7
    for i = 1, segs do
      local u0 = (i - 1) / segs
      local u1 = i / segs
      local r0 = R * (0.12 + 0.84 * u0)
      local r1 = R * (0.12 + 0.84 * u1)
      local a0 = base + u0 * 3.6 * tight
      local a1 = base + u1 * 3.6 * tight
      local alpha = math.floor(200 * (1 - u0 * 0.55) * am2)
      ctx:line(cx + math.cos(a0) * r0, cy + math.sin(a0) * r0,
               cx + math.cos(a1) * r1, cy + math.sin(a1) * r1,
               1.4 + (1 - u0) * 2.2, c2r, c2g, c2b, alpha)
    end
  end

  -- 边缘粒子：沿外圈缓慢滑行，偶尔闪一下。
  local rimN = 14
  for i = 1, rimN do
    local a = i * math.pi * 2 / rimN + phase * 0.45
    local tw = 0.5 + 0.5 * math.sin(phase * 3.1 + i * 1.7)
    ctx:plus_circle(cx + math.cos(a) * R * 0.96, cy + math.sin(a) * R * 0.96,
      1.6 + 1.8 * tw, c2r, c2g, c2b, math.floor(210 * tw * am2))
  end

  ctx:glow(cx, cy, 18 + 26 * glow, c1r, c1g, c1b, math.floor(110 * math.min(1.0, glow) * am1))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
