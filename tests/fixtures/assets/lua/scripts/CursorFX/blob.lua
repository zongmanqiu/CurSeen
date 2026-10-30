-- CursorFX · blob / 复刻自 cursorfx-main/src/cursors/blob.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：一坨软糖跟着指针，头部沿运动方向拉长、侧向压扁，身后拖几颗逐渐变小的
-- 液滴。按下时整体缩小；原版 hover 放大在 CurSeen 没有 hover 命中，省略。
--
-- 实现：头部不是旋转椭圆图元（宿主没有旋转参数），而是把椭圆采样成 20 边多边形
-- 再按 heading 旋转；高光用未旋转的小椭圆近似。液滴链每帧只画 0~8 个圆。
--
-- 性能：默认 4 颗液滴时单帧 4 圆 + 1 个 20 边形 + 1 椭圆 ≈ 6 个图元。

name = "blob"
effect_name = "blob"
kind = "cursor"
description = "CursorFX blob：随速度拉伸的软糖光标，带液滴拖尾。"
hide_cursor = true

local TAU = math.pi * 2.0

function schema()
  return {
    { key = "color",    label = "颜色",   type = "color", alpha = true, default = "#7C5CFF" },
    { key = "size",     label = "大小",   type = "int",   min = 8, max = 60, step = 1, default = 20 },
    { key = "droplets", label = "液滴数", type = "int",   min = 0, max = 8,  step = 1, default = 4 },
  }
end

local pts = {}
local count = 0
local cur_size = 20
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  pts = {}
  count = 0
  cur_size = 20
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    count = math.floor(clampf(params.droplets or 4, 0, 8)) + 1
    for i = 1, count do
      pts[i] = { x = mx, y = my }
    end
    cur_size = params.size or 20
    started = true
  end

  local want = math.floor(clampf(params.droplets or 4, 0, 8)) + 1
  if want > count then
    for i = count + 1, want do
      pts[i] = { x = pts[count].x, y = pts[count].y }
    end
    count = want
  elseif want < count then
    for i = want + 1, count do
      pts[i] = nil
    end
    count = want
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k0 = math.min(1.0, 0.35 * f)
  local k = math.min(1.0, 0.30 * f)
  pts[1].x = pts[1].x + (mx - pts[1].x) * k0
  pts[1].y = pts[1].y + (my - pts[1].y) * k0
  for i = 2, count do
    pts[i].x = pts[i].x + (pts[i - 1].x - pts[i].x) * k
    pts[i].y = pts[i].y + (pts[i - 1].y - pts[i].y) * k
  end

  local target = (params.size or 20) * (mouse_down and 0.75 or 1.0)
  cur_size = cur_size + (target - cur_size) * math.min(1.0, 0.2 * f)
end

function draw(ctx, params)
  if not started or count == 0 then return end

  local cr, cg, cb, ca = hex_to_rgb(params.color or "#7C5CFF")
  local a = math.floor(0.92 * ca)
  local heading = mouse_heading or 0.0
  local speed = mouse_speed or 0.0
  local stretch = 1.0 + clampf(speed * 0.03, 0.0, 0.6)
  local hx, hy = pts[1].x, pts[1].y
  local ch, sh = math.cos(heading), math.sin(heading)
  local size = cur_size

  -- 液滴：从尾到头，半径按链序递减。
  for i = count, 2, -1 do
    local r = size * (1.0 - (i - 1) / (count + 1.0)) * 0.7
    if r > 0.3 then
      ctx:circle(pts[i].x, pts[i].y, r, cr, cg, cb, a)
    end
  end

  -- 头部：旋转椭圆采样。
  local rx = size * stretch
  local ry = size / math.sqrt(stretch)
  local head = {}
  for i = 0, 19 do
    local ang = TAU * (i / 20.0)
    local ex = math.cos(ang) * rx
    local ey = math.sin(ang) * ry
    head[#head + 1] = {
      hx + ex * ch - ey * sh,
      hy + ex * sh + ey * ch,
    }
  end
  ctx:polygon(head, cr, cg, cb, a)

  -- 高光：把原版 (-0.25s, -0.3s) 的局部坐标按 heading 旋到世界坐标。
  local lx, ly = -size * 0.25, -size * 0.30
  local wx = hx + lx * ch - ly * sh
  local wy = hy + lx * sh + ly * ch
  ctx:ellipse(wx, wy, size * 0.28, size * 0.16, 255, 255, 255, 128)
end
