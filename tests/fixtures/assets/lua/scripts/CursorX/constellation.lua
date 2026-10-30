-- CursorX · Constellation
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：整屏漂移的星点互相连线，指针附近的星点再连向指针；指针停在交互
-- 提示上时连接范围与线宽变大，点击时相连的星点闪一下光。星数上限受 CPU 软渲染约束，
-- 默认 90，调到 200 会明显更吃 CPU。

name = "Constellation"
effect_name = "Constellation"
kind = "cursor"
description = "CursorX Constellation：星点连线成网，指针牵引附近的星。"
hide_cursor = true

local stars = {}
local phase = 0
local click_t = -1

function schema()
  return {
    { key = "starCount",    label = "星点数量", type = "int",   min = 30,  max = 200, step = 10,  default = 90 },
    { key = "maxDist",      label = "星间连线", type = "int",   min = 50,  max = 200, step = 5,   default = 115 },
    { key = "cursorDist",   label = "牵引半径", type = "int",   min = 80,  max = 300, step = 5,   default = 175 },
    { key = "starColor",    label = "星点颜色", type = "color", alpha = true, default = "#C8C8FF" },
    { key = "pointerAnim",  label = "悬停扩张", type = "bool",  default = true },
    { key = "pointerDistMult", label = "扩张倍数", type = "float", min = 1.0, max = 2.0, step = 0.05, default = 1.25 },
    { key = "clickAnim",    label = "点击闪星", type = "bool",  default = true },
    { key = "clickGlowIntensity", label = "闪光强度", type = "float", min = 0.5, max = 4.0, step = 0.1, default = 1.5 },
  }
end

function init()
  stars = {}
  phase = 0
  click_t = -1
  local w = screen_w or 1920
  local h = screen_h or 1080
  local ox, oy = screen_x or 0, screen_y or 0
  local n = 90
  for i = 1, n do
    stars[i] = {
      x = ox + rng(0, w), y = oy + rng(0, h),
      vx = rng(-0.14, 0.14), vy = rng(-0.14, 0.14),
      r = rng(0.5, 2.5),
    }
  end
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

local function ensure_count(n)
  local w = screen_w or 1920
  local h = screen_h or 1080
  local ox, oy = screen_x or 0, screen_y or 0
  while #stars < n do
    stars[#stars + 1] = {
      x = ox + rng(0, w), y = oy + rng(0, h),
      vx = rng(-0.14, 0.14), vy = rng(-0.14, 0.14),
      r = rng(0.5, 2.5),
    }
  end
  while #stars > n do table.remove(stars) end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  ensure_count(math.floor(params.starCount or 90))
  local w = screen_w or 1920
  local h = screen_h or 1080
  local ox, oy = screen_x or 0, screen_y or 0
  phase = phase + dt * 3.0

  for i = 1, #stars do
    local s = stars[i]
    s.x = s.x + s.vx * f
    s.y = s.y + s.vy * f
    if s.x < ox then s.x = s.x + w elseif s.x > ox + w then s.x = s.x - w end
    if s.y < oy then s.y = s.y + h elseif s.y > oy + h then s.y = s.y - h end
  end

  if click_t >= 0 then
    click_t = click_t + dt * 3.0
    if click_t >= 1 then click_t = -1 end
  end
end

function draw(ctx, params)
  params = params or {}
  local hover = cursor_hover and params.pointerAnim ~= false
  local max_dist = params.maxDist or 115
  local cursor_dist = (params.cursorDist or 175) * (hover and (params.pointerDistMult or 1.25) or 1.0)
  local pulse = hover and (1.0 + math.sin(phase * 6.0) * 0.25) or 1.0
  local mx, my = mouse_x or 0, mouse_y or 0
  local sr, sg, sb, sa = hex_to_rgb(params.starColor or "#C8C8FF")
  local click_glow = 0
  if click_t >= 0 and params.clickAnim ~= false then
    click_glow = math.sin(click_t * math.pi)
  end

  local n = #stars
  local line_alpha = math.floor(90 * sa / 255)
  for i = 1, n do
    local a = stars[i]
    for j = i + 1, n do
      local b = stars[j]
      local dx, dy = a.x - b.x, a.y - b.y
      local d = math.sqrt(dx * dx + dy * dy)
      if d < max_dist then
        ctx:line(a.x, a.y, b.x, b.y, 0.7, 124, 92, 252,
          math.floor(line_alpha * (1 - d / max_dist)))
      end
    end

    local dcx, dcy = a.x - mx, a.y - my
    local dc = math.sqrt(dcx * dcx + dcy * dcy)
    if dc < cursor_dist then
      ctx:line(a.x, a.y, mx, my, hover and 1.5 * pulse or 1.0, 92, 244, 252,
        math.floor(190 * (1 - dc / cursor_dist) * pulse))
      if click_glow > 0 then
        ctx:plus_circle(a.x, a.y, a.r * (1 + click_glow * (params.clickGlowIntensity or 1.5) * 2.5),
          92, 244, 252, math.floor(200 * click_glow * 0.8))
      end
    end
    ctx:circle(a.x, a.y, a.r, sr, sg, sb, sa)
  end

  -- 指针处画一个小空心环，避免星网里找不到真实指针。
  ctx:ring(mx, my, 9, 1.2, 255, 255, 255, 130)
  ctx:circle(mx, my, 1.6, 255, 255, 255, 230)
end
