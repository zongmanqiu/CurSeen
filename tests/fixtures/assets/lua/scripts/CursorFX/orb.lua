-- CursorFX · orb / 复刻自 cursorfx-main/src/cursors/orb.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：指针处一颗白热核心 + 呼吸辉光，身后拖一条彩虹彗尾；外环有滞后，
-- 按下时爆一圈扩散环。原版 hover 状态在 CurSeen 未开放，环半径只区分按下/常态。
--
-- 性能：尾长默认 40（上限 80），每帧最多 80 个加色圆 + 1 个径向渐变 + 2 个环，
-- 单帧图元约 84 个；没有粒子池，CPU 软渲染下也很轻。

name = "orb"
effect_name = "orb"
kind = "cursor"
description = "CursorFX orb：彩虹彗尾 + 呼吸辉光球，按下时外环脉冲。"
hide_cursor = true

function schema()
  return {
    { key = "length",    label = "彗尾长度", type = "int",   min = 4,   max = 80,  step = 1,    default = 40 },
    { key = "hue_speed", label = "色相速度", type = "float", min = 0,   max = 180, step = 1,    default = 40 },
    { key = "color",     label = "辉光颜色", type = "color", alpha = true, default = "#B4E6FF" },
    { key = "glow",      label = "辉光半径", type = "float", min = 10,  max = 80,  step = 1,    default = 34 },
  }
end

local tx, ty = {}, {}
local count = 0
local ring = { x = 0, y = 0, r = 16, pulse = 0 }
local was_down = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  tx, ty = {}, {}
  count = 0
  ring.x, ring.y, ring.r, ring.pulse = 0, 0, 16, 0
  was_down = false
end

-- 按下时立即点脉冲，不等 step 的边沿检测（复合宿主会转发 on_mouse_down）。
function on_mouse_down(button, x, y, params)
  ring.pulse = 1
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  local target = math.floor(clampf(params.length or 40, 4, 80))
  local f = clampf(dt * 60.0, 0.0, 2.0)

  if count == 0 then
    for i = 1, target do
      tx[i], ty[i] = mx, my
    end
    count = target
    ring.x, ring.y = mx, my
  elseif target > count then
    for i = count + 1, target do
      tx[i], ty[i] = tx[count], ty[count]
    end
    count = target
  elseif target < count then
    for i = target + 1, count do
      tx[i], ty[i] = nil, nil
    end
    count = target
  end

  local k0 = math.min(1.0, 0.6 * f)
  tx[1] = tx[1] + (mx - tx[1]) * k0
  ty[1] = ty[1] + (my - ty[1]) * k0
  local k = math.min(1.0, 0.45 * f)
  for i = 2, count do
    tx[i] = tx[i] + (tx[i - 1] - tx[i]) * k
    ty[i] = ty[i] + (ty[i - 1] - ty[i]) * k
  end

  local kr = math.min(1.0, 0.4 * f)
  ring.x = ring.x + (mx - ring.x) * kr
  ring.y = ring.y + (my - ring.y) * kr

  local down = mouse_down and true or false
  local target_r = down and 10 or 16
  ring.r = ring.r + (target_r - ring.r) * math.min(1.0, 0.25 * f)
  if down and not was_down then
    ring.pulse = 1
  end
  was_down = down
  ring.pulse = ring.pulse * (0.9 ^ f)
end

function draw(ctx, params)
  if count == 0 then return end

  local mx, my = mouse_x or 0, mouse_y or 0
  local time = elapsed or 0
  local hue_speed = params.hue_speed or 40
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#B4E6FF")
  local glow = params.glow or 34

  local base_hue = (time * hue_speed) % 360.0
  for i = count, 1, -1 do
    local k = (i - 1) / count
    local hr, hg, hb = hsv_to_rgb((base_hue + i * 4.0) % 360.0, 0.95, 0.68)
    local a = math.floor(255.0 * (0.55 * (1.0 - k) + 0.05))
    ctx:plus_circle(tx[i], ty[i], (1.0 - k) * 4.5 + 0.6, hr, hg, hb, a)
  end

  local ga = math.floor(150 * (ca / 255.0))
  local gb = math.floor(55 * (ca / 255.0))
  ctx:radial_stops(mx, my, glow, {
    { offset = 0.0,  r = cr, g = cg, b = cb, a = ga },
    { offset = 0.55, r = cr, g = cg, b = cb, a = gb },
    { offset = 1.0,  r = cr, g = cg, b = cb, a = 0 },
  })

  ctx:ring(ring.x, ring.y, math.max(ring.r, 2.0), 1.5, 220, 240, 255, 230)
  if ring.pulse > 0.01 then
    local pr = ring.r + (1.0 - ring.pulse) * 70.0
    ctx:ring(ring.x, ring.y, pr, 2.0, 200, 235, 255,
      math.floor(clampf(ring.pulse * 0.8, 0.0, 1.0) * 255.0))
  end

  ctx:circle(mx, my, 2.6, 255, 255, 255, 255)
end
