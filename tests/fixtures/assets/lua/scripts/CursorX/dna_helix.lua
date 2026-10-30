-- CursorX · DNA Helix
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：记录最近 80 个指针位置，沿轨迹画两条正弦缠绕的彩带与横向碱基对；
-- 指针停在交互提示上时前端圆点放大变透，点击时振幅涌起一次。彩带用带状多边形
-- 绘制（每条一条指令），轨迹采样密度在 step 里控制。

name = "DNA Helix"
effect_name = "DNA Helix"
kind = "cursor"
description = "CursorX DNA Helix：沿指针轨迹缠绕的双螺旋。"
hide_cursor = true

local HIST = 80
local path = {}
local phase = 0
local click_t = -1
local tip_scale = 1.0
local tip_alpha = 1.0

function schema()
  return {
    { key = "color1",      label = "链一颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "color2",      label = "链二颜色", type = "color", alpha = true, default = "#5CF4FC" },
    { key = "amplitude",   label = "螺旋振幅", type = "int",   min = 4,   max = 32,  step = 2,    default = 16 },
    { key = "speed",       label = "波动速度", type = "float", min = 0.03,max = 0.2, step = 0.01, default = 0.09 },
    { key = "pointerAnim", label = "悬停放大", type = "bool",  default = true },
    { key = "pointerScale",label = "放大倍数", type = "float", min = 1.5, max = 5.0, step = 0.1,  default = 3.2 },
    { key = "clickAnim",   label = "点击涌起", type = "bool",  default = true },
    { key = "clickAmpBoost", label = "涌起幅度", type = "int", min = 10,  max = 100, step = 5,    default = 45 },
  }
end

function init()
  path = {}
  phase = 0
  click_t = -1
  tip_scale = 1.0
  tip_alpha = 1.0
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

local function push_point()
  local mx, my = mouse_x or 0, mouse_y or 0
  local last = path[#path]
  -- 位移太小就不采，避免原地抖动把 80 个采样点全耗在同一点上。
  if last then
    local dx, dy = mx - last.x, my - last.y
    if dx * dx + dy * dy < 1.5 then return end
  end
  path[#path + 1] = { x = mx, y = my }
  while #path > HIST do table.remove(path, 1) end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  phase = phase + (params.speed or 0.09) * f
  push_point()

  local hover = cursor_hover and params.pointerAnim ~= false
  local target_scale = hover and (params.pointerScale or 3.2) or 1.0
  local target_alpha = hover and 0.45 or 1.0
  tip_scale = tip_scale + (target_scale - tip_scale) * math.min(1.0, 0.15 * f)
  tip_alpha = tip_alpha + (target_alpha - tip_alpha) * math.min(1.0, 0.15 * f)

  if click_t >= 0 then
    click_t = click_t + dt * 3.0
    if click_t >= 1 then click_t = -1 end
  end
end

-- 沿轨迹生成一条链的带状多边形（用切线法线偏移正弦波）。
local function strand_polygon(phase_off, amp, boost, width)
  local n = #path
  if n < 2 then return nil end
  local left, right = {}, {}
  for i = 1, n do
    local p = path[i]
    local a = path[math.max(1, i - 1)]
    local b = path[math.min(n, i + 1)]
    local dx, dy = b.x - a.x, b.y - a.y
    local len = math.sqrt(dx * dx + dy * dy)
    if len < 1e-3 then dx, dy, len = 1, 0, 1 end
    local nx, ny = -dy / len, dx / len
    local t = i / n
    local off = math.sin(phase * 0.65 + i * 0.2 + phase_off) * (amp * t + boost * t)
    local w = width * 0.5
    left[i] = { p.x + nx * (off + w), p.y + ny * (off + w) }
    right[n - i + 1] = { p.x + nx * (off - w), p.y + ny * (off - w) }
  end
  local poly = {}
  for i = 1, n do poly[#poly + 1] = left[i] end
  for i = 1, n do poly[#poly + 1] = right[i] end
  return poly
end

function draw(ctx, params)
  params = params or {}
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#7C5CFC")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#5CF4FC")
  local mx, my = mouse_x or 0, mouse_y or 0
  local dot = math.max(1.0, 4.0 * tip_scale)
  ctx:circle(mx, my, dot + 1.4, 0, 0, 0, math.floor(110 * tip_alpha))
  ctx:circle(mx, my, dot, 255, 255, 255, math.floor(255 * tip_alpha))

  if #path < 4 then return end
  local amp = params.amplitude or 16
  local boost = 0
  if click_t >= 0 and params.clickAnim ~= false then
    boost = math.sin(click_t * math.pi) * (params.clickAmpBoost or 45)
  end

  local p1 = strand_polygon(0, amp, boost, 2.0)
  local p2 = strand_polygon(math.pi, amp, boost, 2.0)
  if p1 then ctx:polygon(p1, c1r, c1g, c1b, c1a) end
  if p2 then ctx:polygon(p2, c2r, c2g, c2b, c2a) end

  -- 每 10 个采样画一根横向碱基对。
  local n = #path
  for i = 1, n, 10 do
    local p = path[i]
    local a = path[math.max(1, i - 1)]
    local b = path[math.min(n, i + 1)]
    local dx, dy = b.x - a.x, b.y - a.y
    local len = math.sqrt(dx * dx + dy * dy)
    if len < 1e-3 then dx, dy, len = 1, 0, 1 end
    local nx, ny = -dy / len, dx / len
    local t = i / n
    local off = math.sin(phase * 0.65 + i * 0.2) * (amp * t + boost * t)
    local x1 = p.x + nx * off
    local y1 = p.y + ny * off
    local x2 = p.x + nx * math.sin(phase * 0.65 + i * 0.2 + math.pi) * (amp * t + boost * t)
    local y2 = p.y + ny * math.sin(phase * 0.65 + i * 0.2 + math.pi) * (amp * t + boost * t)
    ctx:line(x1, y1, x2, y2, 1.0, 200, 200, 255, 56)
  end
end
