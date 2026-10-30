-- MouseEffects · Hearts
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围上浮的爱心：两个圆 + 一个三角拼成心形，边升边摆边轻微旋转。
-- 上游 11 个参数；这里保留数量、尺寸、上浮、摆动、旋转、颜色模式六项。

name = "Hearts"
effect_name = "Hearts"
kind = "cursor"
description = "MouseEffects Hearts：围绕光标上浮的爱心。"
hide_cursor = true

local MAX_HEARTS = 30
local hearts = {}
local cx, cy = 0, 0
local started = false

local PALETTE = {
  { 255, 77, 109 }, { 255, 128, 171 }, { 255, 42, 94 },
  { 255, 170, 200 }, { 214, 51, 108 },
}

function schema()
  return {
    { key = "count",  label = "爱心数",   type = "int",   min = 6,   max = 30,  step = 1,   default = 15 },
    { key = "size",   label = "爱心大小", type = "float", min = 10,  max = 36,  step = 1,   default = 22 },
    { key = "float",  label = "上浮速度", type = "float", min = 20,  max = 90,  step = 5,   default = 40 },
    { key = "wobble", label = "摆动幅度", type = "float", min = 0,   max = 60,  step = 5,   default = 30 },
    { key = "rotate", label = "旋转幅度", type = "float", min = 0,   max = 0.6, step = 0.05, default = 0.2 },
    { key = "mode",   label = "配色",     type = "choice", choices = { "pink", "red", "rainbow" },
      labels = { "粉色", "红色", "彩虹" }, default = "pink" },
    { key = "glow",   label = "辉光强度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.8 },
  }
end

function init()
  hearts = {}
  for i = 1, MAX_HEARTS do
    hearts[i] = {
      dx = rng(-180, 180),
      dy = rng(-160, 240),
      phase = rng(0, math.pi * 2),
      size = rng(0.65, 1.25),
      color = (i - 1) % #PALETTE + 1,
      spin = rng(-0.6, 0.6),
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
  local k = math.min(1.0, 0.10 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local up = params.float or 40
  local wob = params.wobble or 30
  for i = 1, #hearts do
    local h = hearts[i]
    h.phase = h.phase + dt * 1.6
    h.dy = h.dy - up * dt
    h.dx = h.dx + math.sin(h.phase) * wob * dt
    if h.dy < -240 then
      h.dy = 240
      h.dx = rng(-180, 180)
    end
    if h.dx > 240 then h.dx = -220 elseif h.dx < -240 then h.dx = 220 end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_HEARTS, math.floor(params.count or 15))
  local base = params.size or 22
  local rotAmp = params.rotate or 0.2
  local mode = params.mode or "pink"
  local glow = params.glow or 0.8

  for i = 1, n do
    local h = hearts[i]
    local s = base * h.size
    local x = cx + h.dx
    local y = cy + h.dy
    local rot = math.sin(h.phase * 0.7) * rotAmp + h.spin * 0.2
    local ca, sa = math.cos(rot), math.sin(rot)

    local c
    if mode == "red" then
      c = PALETTE[(i % 2) + 2]
    elseif mode == "rainbow" then
      c = { math.floor(127 + 127 * math.sin(h.phase)), math.floor(127 + 127 * math.sin(h.phase + 2.1)), math.floor(127 + 127 * math.sin(h.phase + 4.2)) }
    else
      c = PALETTE[(i - 1) % #PALETTE + 1]
    end

    local function rp(px, py)
      return x + px * ca - py * sa, y + px * sa + py * ca
    end
    local c1x, c1y = rp(-s * 0.28, -s * 0.20)
    local c2x, c2y = rp(s * 0.28, -s * 0.20)
    ctx:circle(c1x, c1y, s * 0.34, c[1], c[2], c[3], 215)
    ctx:circle(c2x, c2y, s * 0.34, c[1], c[2], c[3], 215)
    local p1x, p1y = rp(-s * 0.62, -s * 0.02)
    local p2x, p2y = rp(s * 0.62, -s * 0.02)
    local p3x, p3y = rp(0, s * 0.78)
    ctx:polygon({ { p1x, p1y }, { p2x, p2y }, { p3x, p3y } }, c[1], c[2], c[3], 215)
    if glow > 0.01 and i % 3 == 0 then
      ctx:glow(x, y, s * 0.9, c[1], c[2], c[3], math.floor(45 * glow))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
