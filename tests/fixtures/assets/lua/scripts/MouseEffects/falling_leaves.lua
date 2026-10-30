-- MouseEffects · Falling Leaves
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围飘落的秋叶：六点叶形 + 叶脉，边落边摆边翻转；颜色在红/橙/黄/褐之间变化。
-- 上游 11 个参数；这里保留数量、尺寸、落速、风力、摆动、翻转、色彩变化七项。

name = "Falling Leaves"
effect_name = "Falling Leaves"
kind = "cursor"
description = "MouseEffects Falling Leaves：围绕光标翻落的秋叶。"
hide_cursor = true

local MAX_LEAVES = 60
local leaves = {}
local cx, cy = 0, 0
local started = false

local PALETTE = {
  { 214, 69, 42 }, { 235, 128, 48 }, { 244, 184, 64 },
  { 168, 86, 38 },  { 196, 54, 78 }, { 222, 148, 56 },
}

function schema()
  return {
    { key = "count",    label = "叶片数",   type = "int",   min = 12,  max = 60,  step = 2,   default = 30 },
    { key = "size",     label = "叶片大小", type = "float", min = 10,  max = 40,  step = 1,   default = 22 },
    { key = "fall",     label = "下落速度", type = "float", min = 20,  max = 100, step = 5,   default = 45 },
    { key = "wind",     label = "风力",     type = "float", min = 0,   max = 80,  step = 5,   default = 25 },
    { key = "sway",     label = "摆动幅度", type = "float", min = 0,   max = 80,  step = 5,   default = 40 },
    { key = "spin",     label = "翻转速度", type = "float", min = 0,   max = 4,   step = 0.1, default = 2.0 },
    { key = "variety",  label = "色彩变化", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.8 },
  }
end

function init()
  leaves = {}
  for i = 1, MAX_LEAVES do
    leaves[i] = {
      dx = rng(-260, 260),
      dy = rng(-260, 260),
      phase = rng(0, math.pi * 2),
      rot = rng(0, math.pi * 2),
      spin = rng(-1, 1),
      size = rng(0.7, 1.3),
      color = (i - 1) % #PALETTE + 1,
    }
  end
  cx, cy = mouse_x or 0, mouse_y or 0
  started = false
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  if not started then
    cx, cy = mx, my
    started = true
  end
  local k = math.min(1.0, 0.08 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local fall = params.fall or 45
  local wind = params.wind or 25
  local sway = params.sway or 40
  local spin = params.spin or 2.0
  for i = 1, #leaves do
    local l = leaves[i]
    l.phase = l.phase + dt * (0.8 + spin * 0.5)
    l.rot = l.rot + l.spin * spin * dt
    l.dy = l.dy + fall * dt
    l.dx = l.dx + (wind * 0.4 + math.sin(l.phase) * sway) * dt
    if l.dy > 300 then
      l.dy = -280
      l.dx = rng(-260, 260)
    end
    if l.dx > 320 then l.dx = -300 elseif l.dx < -320 then l.dx = 300 end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_LEAVES, math.floor(params.count or 30))
  local base = params.size or 22
  local variety = params.variety or 0.8
  local t = elapsed or 0

  for i = 1, n do
    local l = leaves[i]
    local s = base * l.size
    local x = cx + l.dx
    local y = cy + l.dy
    local flip = 0.22 + 0.78 * math.abs(math.cos(l.phase * 0.8))
    local ca, sa = math.cos(l.rot), math.sin(l.rot)
    local shape = {
      { 0, -s * 0.62 },
      { s * 0.52 * flip, -s * 0.22 },
      { s * 0.40 * flip, s * 0.50 },
      { 0, s * 0.70 },
      { -s * 0.40 * flip, s * 0.50 },
      { -s * 0.52 * flip, -s * 0.22 },
    }
    local pts = {}
    for k = 1, #shape do
      local px, py = shape[k][1], shape[k][2]
      pts[k] = { x + px * ca - py * sa, y + px * sa + py * ca }
    end
    local c = PALETTE[l.color]
    local mixk = (1 - variety) * (0.5 + 0.5 * math.sin(t * 0.7 + i))
    local cr = math.floor(c[1] * (0.8 + 0.2 * mixk))
    local cg = math.floor(c[2] * (0.8 + 0.2 * mixk))
    local cb = math.floor(c[3] * (0.8 + 0.2 * mixk))
    ctx:polygon(pts, cr, cg, cb, 215)
    -- 叶脉：从叶柄到叶尖。
    ctx:line(x, y - s * 0.55, x, y + s * 0.58, 0.7,
      math.floor(cr * 0.55), math.floor(cg * 0.45), math.floor(cb * 0.35), 130)
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
