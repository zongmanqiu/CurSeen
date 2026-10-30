-- CursorX · Mirror Ghost
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：以屏幕中心为镜面，四个方向的镜像残影跟着指针平滑移动；指针停在
-- 交互提示上时残影放大、中心点缩小，点击时残影收缩、白点弹大。

name = "Mirror Ghost"
effect_name = "Mirror Ghost"
kind = "cursor"
description = "CursorX Mirror Ghost：屏幕中心镜像的四残影光标。"
hide_cursor = true

local ghosts = {}
local ghost_scale = 1.0
local dot_scale = 1.0
local click_t = -1

function schema()
  return {
    { key = "color",       label = "残影颜色", type = "color", alpha = true, default = "#C45CFC" },
    { key = "size",        label = "圆环大小", type = "int",   min = 10,  max = 40,  step = 2,    default = 20 },
    { key = "lerp",        label = "跟随速度", type = "float", min = 0.06,max = 0.4, step = 0.02, default = 0.14 },
    { key = "pointerAnim", label = "悬停缩放", type = "bool",  default = true },
    { key = "pointerScale",label = "悬停倍数", type = "float", min = 1.2, max = 4.5, step = 0.1,  default = 2.2 },
    { key = "clickAnim",   label = "点击脉冲", type = "bool",  default = true },
    { key = "clickScale",  label = "点击幅度", type = "float", min = 0.2, max = 2.0, step = 0.1,  default = 0.8 },
  }
end

function init()
  local mx, my = mouse_x or 0, mouse_y or 0
  ghosts = {
    { x = mx, y = my, sx = 1,  sy = 1,  l = 1.0,  alpha = 0.9,  size = 0 },
    { x = mx, y = my, sx = -1, sy = 1,  l = 0.14, alpha = 0.5,  size = -2 },
    { x = mx, y = my, sx = 1,  sy = -1, l = 0.14, alpha = 0.5,  size = -2 },
    { x = mx, y = my, sx = -1, sy = -1, l = 0.098,alpha = 0.28, size = -4 },
  }
  ghost_scale = 1.0
  dot_scale = 1.0
  click_t = -1
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local base_lerp = params.lerp or 0.14
  local hover = cursor_hover and params.pointerAnim ~= false
  local cx = (screen_x or 0) + (screen_w or 1920) * 0.5
  local cy = (screen_y or 0) + (screen_h or 1080) * 0.5
  local mx, my = mouse_x or cx, mouse_y or cy

  local target_ghost, target_dot = 1.0, 1.0
  if hover then
    target_ghost = params.pointerScale or 2.2
    target_dot = 0.5
  end
  if click_t >= 0 and params.clickAnim ~= false then
    local bounce = math.sin(click_t * math.pi)
    local cs = params.clickScale or 0.8
    target_ghost = target_ghost * (1 - bounce * cs * 0.5)
    target_dot = target_dot * (1 + bounce * cs * 1.2)
    click_t = click_t + dt * 4.8
    if click_t >= 1 then click_t = -1 end
  end
  ghost_scale = ghost_scale + (target_ghost - ghost_scale) * math.min(1.0, 0.15 * f)
  dot_scale = dot_scale + (target_dot - dot_scale) * math.min(1.0, 0.15 * f)

  for i = 1, #ghosts do
    local g = ghosts[i]
    local tx = cx + (mx - cx) * g.sx
    local ty = cy + (my - cy) * g.sy
    g.l = (i == 4) and base_lerp * 0.7 or (i == 1 and 1.0 or base_lerp)
    local k = 1.0 - math.pow(1.0 - g.l, f)
    g.x = g.x + (tx - g.x) * k
    g.y = g.y + (ty - g.y) * k
  end
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, a = hex_to_rgb(params.color or "#C45CFC")
  local base = params.size or 20

  for i = 1, #ghosts do
    local gh = ghosts[i]
    local size = math.max(1.0, (base + gh.size) * ghost_scale)
    local al = math.floor(a * gh.alpha)
    ctx:ring(gh.x, gh.y, size * 0.5 + 1.6, 3.0, r, g, b, math.floor(al * 0.16))
    ctx:ring(gh.x, gh.y, size * 0.5, 2.0, r, g, b, al)
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  local dot = math.max(0.5, 3.0 * dot_scale)
  ctx:circle(mx, my, dot + 1.2, 0, 0, 0, 130)
  ctx:circle(mx, my, dot, 255, 255, 255, 255)
end
