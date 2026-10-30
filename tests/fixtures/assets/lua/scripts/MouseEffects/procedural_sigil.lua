-- MouseEffects · Procedural Sigil
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的程序化法阵：外圈刻度、反向旋转的环、中央双星形与 12 个符文刻痕。
-- 上游 55 个参数/多层 GPU 线框；这里保留最可读的骨架，参数压到 7 个。

name = "Procedural Sigil"
effect_name = "Procedural Sigil"
kind = "cursor"
description = "MouseEffects Procedural Sigil：旋转法阵、星形与符文刻痕。"
hide_cursor = true

local GLYPHS = {
  { { -1, -1, 1, 1 }, { -1, 1, 1, -1 } },
  { { 0, -1, 0, 1 }, { -1, 0, 1, 0 } },
  { { -1, -1, -1, 1 }, { -1, 1, 1, 0 } },
  { { -1, -1, 1, -1 }, { 1, -1, 0, 1 } },
  { { -1, 0, 1, 0 }, { 0, -1, 0, 1 }, { -1, -1, 1, 1 } },
  { { -1, 1, 0, -1 }, { 0, -1, 1, 1 } },
  { { -1, -1, 0, 0 }, { 0, 0, 1, 1 }, { 1, -1, -1, 1 } },
  { { 0, -1, -1, 0 }, { -1, 0, 0, 1 }, { 0, 1, 1, 0 } },
  { { -1, -1, -1, 1 }, { 1, -1, 1, 1 } },
  { { -1, 0, 0, -1 }, { 0, -1, 1, 0 }, { 1, 0, 0, 1 } },
  { { -1, -1, 1, 1 }, { -1, 1, 0, 0 } },
  { { 0, -1, -1, 1 }, { -1, 1, 1, 0 } },
}

local phase = 0
local cx, cy = 0, 0

function schema()
  return {
    { key = "radius",  label = "法阵半径", type = "float", min = 80,  max = 300, step = 10, default = 180 },
    { key = "rings",   label = "环数",     type = "int",   min = 2,   max = 4,   step = 1,  default = 3 },
    { key = "points",  label = "星形顶点", type = "int",   min = 5,   max = 12,  step = 1,  default = 7 },
    { key = "spin",    label = "旋转速度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.5 },
    { key = "pulse",   label = "核心脉动", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.0 },
    { key = "glow",    label = "辉光强度", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.4 },
    { key = "color1",  label = "主色",     type = "color", default = "#00CC66" },
    { key = "color2",  label = "辅色",     type = "color", default = "#88AA00" },
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
  phase = phase + dt * (params.spin or 0.5)
end

function draw(ctx, params)
  params = params or {}
  local R = params.radius or 180
  local rings = math.max(2, math.min(4, math.floor(params.rings or 3)))
  local points = math.max(5, math.min(12, math.floor(params.points or 7)))
  local glow = params.glow or 1.4
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#00CC66")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#88AA00")
  local am1, am2 = c1a / 255, c2a / 255

  -- 外圈与 48 道刻度。
  ctx:ring(cx, cy, R, 1.4, c2r, c2g, c2b, math.floor(190 * am2))
  for i = 0, 47 do
    local a = i * math.pi * 2 / 48
    local r0 = (i % 4 == 0) and R * 0.92 or R * 0.955
    ctx:line(cx + math.cos(a) * r0, cy + math.sin(a) * r0,
             cx + math.cos(a) * R, cy + math.sin(a) * R,
             1.0, c2r, c2g, c2b, math.floor(((i % 4 == 0) and 150 or 90) * am2))
  end

  -- 反向旋转的环 + 环上节点。
  for i = 1, rings do
    local rr = R * (0.48 + 0.15 * i)
    local dir = (i % 2 == 0) and 1 or -1
    local a0 = phase * 0.8 * dir
    ctx:ring(cx, cy, rr, 1.1, c1r, c1g, c1b, math.floor(150 * am1))
    for k = 0, 5 do
      local a = a0 + k * math.pi / 3
      ctx:plus_circle(cx + math.cos(a) * rr, cy + math.sin(a) * rr,
        2.2, c1r, c1g, c1b, math.floor(190 * am1))
    end
  end

  -- 中央双星形：顶点交替连线，第二层旋转半个角。
  for layer = 0, 1 do
    local off = layer * math.pi / points + phase * 0.5
    for i = 0, points - 1 do
      local a1 = off + i * math.pi * 2 / points
      local a2 = off + (i + 2) * math.pi * 2 / points
      local rr = R * (layer == 0 and 0.36 or 0.25)
      ctx:line(cx + math.cos(a1) * rr, cy + math.sin(a1) * rr,
               cx + math.cos(a2) * rr, cy + math.sin(a2) * rr,
               layer == 0 and 1.4 or 1.0, c1r, c1g, c1b,
               math.floor((layer == 0 and 190 or 110) * am1))
    end
  end

  -- 12 个符文刻痕：小角度折线，跟随外圈旋转。
  for i = 1, 12 do
    local a = i * math.pi * 2 / 12 + phase * 0.8
    local bx = cx + math.cos(a) * R * 0.84
    local by = cy + math.sin(a) * R * 0.84
    local g = GLYPHS[i]
    local s = 5.0
    local ca, sa = math.cos(a), math.sin(a)
    for _, seg in ipairs(g) do
      local u1, v1, u2, v2 = seg[1], seg[2], seg[3], seg[4]
      ctx:line(bx + (u1 * ca - v1 * sa) * s, by + (u1 * sa + v1 * ca) * s,
               bx + (u2 * ca - v2 * sa) * s, by + (u2 * sa + v2 * ca) * s,
               0.9, c2r, c2g, c2b, math.floor(160 * am2))
    end
  end

  local pulse = 3 + 3 * (0.5 + 0.5 * math.sin(phase * 3.0 * (params.pulse or 1.0)))
  ctx:plus_circle(cx, cy, pulse, c1r, c1g, c1b, math.floor(230 * am1))
  ctx:glow(cx, cy, 14 + 18 * glow, c1r, c1g, c1b, math.floor(70 * math.min(1.0, glow) * am1))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
