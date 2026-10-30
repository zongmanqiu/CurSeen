-- CursorFX · ghost / 复刻自 cursorfx-main/src/cursors/ghost.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：一只半透明小幽灵用很软的弹簧跟在指针后面，上下轻浮；移动时在身后留下
-- 逐渐淡出的残影，裙摆按正弦波动。眼睛朝指针运动方向偏。
-- 原版 hover 只在按下时把身体缩小，这里同样只区分 mouse_down。
--
-- 性能：身体是约 29 点多边形 + 两只椭圆眼；残影上限 10 个。默认 6 个残影时
-- 单帧约 7 个多边形 + 14 个椭圆，远低于粒子上限。

name = "ghost"
effect_name = "ghost"
kind = "cursor"
description = "CursorFX ghost：漂浮小幽灵，移动时留下淡残影。"
hide_cursor = true

function schema()
  return {
    { key = "color",   label = "颜色",   type = "color", alpha = true, default = "#F0F4FFEB" },
    { key = "size",    label = "大小",   type = "int",   min = 14, max = 60, step = 1, default = 26 },
    { key = "echoes",  label = "残影数", type = "int",   min = 0,  max = 10, step = 1, default = 6 },
  }
end

local px, py = 0, 0
local echoes = {}
local echo_timer = 0
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  px, py = 0, 0
  echoes = {}
  echo_timer = 0
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    px, py = mx, my
    started = true
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k = math.min(1.0, 0.14 * f)
  px = px + (mx - px) * k
  py = py + (my - py) * k

  echo_timer = echo_timer + dt
  local want = math.floor(clampf(params.echoes or 6, 0, 10))
  if echo_timer > 0.06 and (mouse_speed or 0) > 1.5 and want > 0 then
    echo_timer = 0
    echoes[#echoes + 1] = { x = px, y = py, life = 1.0 }
    while #echoes > want do
      table.remove(echoes, 1)
    end
  end

  for i = #echoes, 1, -1 do
    local e = echoes[i]
    e.life = e.life - 0.035 * f
    if e.life <= 0 then
      table.remove(echoes, i)
    end
  end
end

-- 幽灵轮廓：上半圆 + 右侧直落 + 四个裙摆波 + 闭合回左上。
local function ghost_points(x, y, s, wob)
  local pts = {}
  for i = 0, 12 do
    local a = math.pi + math.pi * (i / 12.0)
    pts[#pts + 1] = {
      x + math.cos(a) * s * 0.5,
      y - s * 0.15 + math.sin(a) * s * 0.5,
    }
  end
  pts[#pts + 1] = { x + s * 0.5, y + s * 0.45 }

  for i = 0, 3 do
    local sx = s * 0.5 - i * (s * 0.25)
    local ex = sx - s * 0.25
    local cx = sx + s * 0.125
    local cy = s * 0.45 + ((i % 2 == 0) and 1 or -1) * s * 0.12 + math.sin(wob + i) * s * 0.05
    for j = 1, 4 do
      local u = j / 4.0
      local v = 1.0 - u
      local qx = v * v * sx + 2.0 * v * u * cx + u * u * ex
      local qy = v * v * (s * 0.45) + 2.0 * v * u * cy + u * u * (s * 0.45)
      pts[#pts + 1] = { x + qx, y + qy }
    end
  end
  return pts
end

local function draw_ghost(ctx, x, y, s, alpha, look_x, look_y, wob, cr, cg, cb, ca)
  local a = math.floor(255.0 * clampf(alpha, 0.0, 1.0) * (ca / 255.0))
  if a <= 0 then return end

  ctx:polygon(ghost_points(x, y, s, wob), cr, cg, cb, a)

  local ex = look_x * s * 0.08
  local ey = look_y * s * 0.06
  ctx:ellipse(x - s * 0.17 + ex, y - s * 0.18 + ey, s * 0.07, s * 0.10, 27, 29, 42, a)
  ctx:ellipse(x + s * 0.17 + ex, y - s * 0.18 + ey, s * 0.07, s * 0.10, 27, 29, 42, a)
end

function draw(ctx, params)
  if not started then return end

  local mx, my = mouse_x or 0, mouse_y or 0
  local time = elapsed or 0
  local size = params.size or 26
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#F0F4FFEB")
  local look_x = clampf((mouse_vx or 0) / 12.0, -1.0, 1.0)
  local look_y = clampf((mouse_vy or 0) / 12.0, -1.0, 1.0)
  local wob = time * 6.0

  for i = 1, #echoes do
    local e = echoes[i]
    draw_ghost(ctx, e.x, e.y, size, e.life * 0.25, look_x, look_y, wob, cr, cg, cb, ca)
  end

  local bob = math.sin(time * 3.0) * 3.0
  local s = size * (mouse_down and 0.85 or 1.0)
  draw_ghost(ctx, px, py + bob, s, 1.0, look_x, look_y, wob, cr, cg, cb, ca)
end
