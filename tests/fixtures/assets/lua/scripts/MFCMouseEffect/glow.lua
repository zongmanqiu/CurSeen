-- MFCMouseEffect · Glow
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 呼吸灯：指针静止 1500ms 后出现，四向准星刻度 + 一个绕行圆点 + 呼吸脉冲。
-- 上游名字叫 Glow，但视觉是准星而不是单纯光晕。任何移动/点击立即隐藏。

name = "Glow"
effect_name = "Glow"
kind = "hover"
description = "MFCMouseEffect Glow：静止后出现的呼吸准星。"

local IDLE_MS = 1500
local last_activity = 0
local vis = 0.0

function schema()
  return {
    { key = "color",    label = "准星颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "glow",     label = "辉光颜色", type = "color", alpha = true, default = "#FF5AF766" },
    { key = "size",     label = "准星半径", type = "float", min = 20, max = 140, step = 2,   default = 50 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 6,   step = 0.1, default = 2.2 },
    { key = "duration", label = "呼吸周期(ms)", type = "int", min = 800, max = 6000, step = 50, default = 2600 },
  }
end

local function activity()
  last_activity = wall_ms or 0
  vis = 0.0
end

function init()
  last_activity = wall_ms or 0
  vis = 0.0
end

function on_move(x, y, t, params) activity() end
function on_mouse_down(button, x, y, params) activity() end
function on_mouse_up(button, x, y, params) activity() end
function on_wheel(delta, horizontal, x, y, params) activity() end

function step(dt, t, params)
  local now = wall_ms or 0
  local target = 0.0
  if not mouse_down and now - last_activity >= IDLE_MS then target = 1.0 end
  local k = math.min(1.0, dt * 4.0)
  vis = vis + (target - vis) * k
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, a = hex_to_rgb(params.color or "#FF5AF7")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#FF5AF766")
  local radius = params.size or 50
  local width = params.width or 2.2
  local duration = math.max(800, params.duration or 2600)
  local now = wall_ms or 0
  local phase = ((now - last_activity - IDLE_MS) % duration) / duration
  local pulse = 0.4 + 0.6 * math.sin(phase * math.pi * 2)
  local alpha = clamp(pulse, 0, 1) * vis
  if alpha <= 0.01 then return end
  local mx, my = mouse_x or 0, mouse_y or 0
  local tick = radius * 0.22
  local aa = math.floor(a * alpha)
  local ga2 = math.floor(ga * alpha * 0.6)

  local lines = {
    { mx - radius, my, mx - radius + tick, my },
    { mx + radius - tick, my, mx + radius, my },
    { mx, my - radius, mx, my - radius + tick },
    { mx, my + radius - tick, mx, my + radius },
  }
  for _ = 0, 1 do
    local col = _ == 0 and { gr, gg, gb, ga2 } or { r, g, b, aa }
    local w = _ == 0 and width + 6.0 or width + 0.5
    for i = 1, 4 do
      local l = lines[i]
      ctx:line(l[1], l[2], l[3], l[4], w, col[1], col[2], col[3], col[4])
    end
  end

  local dot_angle = phase * math.pi * 2
  local dot_r = width * 1.6
  ctx:circle(mx + math.cos(dot_angle) * radius * 0.6,
    my + math.sin(dot_angle) * radius * 0.6, dot_r,
    r, g, b, math.floor(a * alpha))
end
