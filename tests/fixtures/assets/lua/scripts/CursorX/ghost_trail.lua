-- CursorX · Ghost Trail
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：弹簧物理的小幽灵跟在指针后：身体是会摆动的圆头 + 波浪裙摆，两只手臂
-- 交替挥动，眼睛朝移动方向看；身下持续飘出紫色尘粒。指针停在交互提示上时整体放大，
-- 点击弹一下。

name = "Ghost Trail"
effect_name = "Ghost Trail"
kind = "cursor"
description = "CursorX Ghost Trail：弹簧跟随的招手小幽灵。"
hide_cursor = true

local MAX_DUST = 160
local gx, gy = 0, 0
local vx, vy = 0, 0
local time = 0
local click_t = -1
local dust = {}

function schema()
  return {
    { key = "color",        label = "幽灵颜色", type = "color", alpha = true, default = "#FFFFFF" },
    { key = "glowColor",    label = "辉光颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "size",         label = "幽灵大小", type = "int",   min = 15,  max = 45,  step = 1,    default = 22 },
    { key = "stiffness",    label = "弹簧刚度", type = "float", min = 0.04,max = 0.4, step = 0.02, default = 0.12 },
    { key = "damping",      label = "阻尼",     type = "float", min = 0.5, max = 0.95,step = 0.02, default = 0.78 },
    { key = "glowRadius",   label = "辉光半径", type = "int",   min = 0,   max = 40,  step = 1,    default = 15 },
    { key = "particleCount",label = "尘粒数量", type = "int",   min = 0,   max = 10,  step = 1,    default = 3 },
    { key = "pointerAnim",  label = "悬停放大", type = "bool",  default = true },
    { key = "pointerScale", label = "悬停倍数", type = "float", min = 1.1, max = 2.5, step = 0.05, default = 1.35 },
    { key = "clickAnim",    label = "点击弹跳", type = "bool",  default = true },
    { key = "clickScale",   label = "弹跳幅度", type = "float", min = 0.2, max = 1.2, step = 0.05, default = 0.5 },
  }
end

function init()
  local mx, my = mouse_x or 0, mouse_y or 0
  gx, gy = mx, my
  vx, vy = 0, 0
  time = 0
  click_t = -1
  dust = {}
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  time = time + dt * 3.0
  local mx, my = mouse_x or gx, mouse_y or gy
  local stiffness = params.stiffness or 0.12
  local damping = params.damping or 0.78

  local ax = (mx - gx) * stiffness * f
  local ay = (my - gy) * stiffness * f
  vx = (vx + ax) * math.pow(damping, f)
  vy = (vy + ay) * math.pow(damping, f)
  gx = gx + vx * f
  gy = gy + vy * f

  local hover = cursor_hover and params.pointerAnim ~= false
  local p_scale = hover and (params.pointerScale or 1.35) or 1.0
  local click_mul = 1.0
  if click_t >= 0 and params.clickAnim ~= false then
    click_mul = 1.0 + math.sin(click_t * math.pi) * (params.clickScale or 0.5)
    click_t = click_t + dt * 3.6
    if click_t >= 1 then click_t = -1 end
  end
  local size = (params.size or 22) * p_scale * click_mul

  local rate = params.particleCount or 3
  if rate > 0 then
    local want = rate * 0.15 * f
    local n = math.floor(want)
    if rng(0, 1) < want - n then n = n + 1 end
    for _ = 1, n do
      if #dust < MAX_DUST then
        dust[#dust + 1] = {
          x = gx + rng(-0.75, 0.75) * size * 1.5,
          y = gy + size * 1.2,
          vx = rng(-0.5, 0.5) - vx * 0.1,
          vy = rng(0.5, 1.7),
          life = 1.0,
          size = rng(2, 5),
          hue = 0,
        }
      end
    end
  end

  for i = #dust, 1, -1 do
    local p = dust[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    p.life = p.life - 0.02 * f
    if p.life <= 0 then table.remove(dust, i) end
  end
end

-- 幽灵身体：上半圆 + 两侧 + 波浪裙摆，全部在幽灵局部坐标里生成后旋转/平移。
local function ghost_body(cx, cy, r, tilt, t)
  local pts = {}
  -- 上半圆：角度 π → 2π（屏幕坐标里 y 向上）。
  for i = 0, 16 do
    local a = math.pi + math.pi * i / 16
    pts[#pts + 1] = { math.cos(a) * r, math.sin(a) * r }
  end
  local body_h = r * 1.8
  pts[#pts + 1] = { r, body_h }
  local wave_amp = r * 0.15
  for i = 0, 20 do
    local u = i / 20
    local x = r - u * r * 2
    local y = body_h + math.sin(t * 2.8 + u * math.pi * 3) * wave_amp
    pts[#pts + 1] = { x, y }
  end
  pts[#pts + 1] = { -r, body_h }

  local ca, sa = math.cos(tilt), math.sin(tilt)
  for i = 1, #pts do
    local x, y = pts[i][1], pts[i][2]
    pts[i] = { cx + x * ca - y * sa, cy + x * sa + y * ca }
  end
  return pts
end

local function arm(cx, cy, r, tilt, side, angle)
  local ca, sa = math.cos(tilt), math.sin(tilt)
  local ax, ay = -r * 0.85 * side, r * 0.4
  local aca, asa = math.cos(angle), math.sin(angle)
  local rx, ry = r * 0.5, r * 0.22
  local pts = {}
  for i = 0, 12 do
    local t = i / 12 * math.pi * 2
    local ex, ey = math.cos(t) * rx, math.sin(t) * ry
    local lx = ax + ex * aca - ey * asa
    local ly = ay + ex * asa + ey * aca
    pts[#pts + 1] = { cx + lx * ca - ly * sa, cy + lx * sa + ly * ca }
  end
  return pts
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#FFFFFF")
  local gr, gg, gb, ga = hex_to_rgb(params.glowColor or "#7C5CFC")
  local hover = cursor_hover and params.pointerAnim ~= false
  local p_scale = hover and (params.pointerScale or 1.35) or 1.0
  local click_mul = 1.0
  if click_t >= 0 and params.clickAnim ~= false then
    click_mul = 1.0 + math.sin(click_t * math.pi) * (params.clickScale or 0.5)
  end
  local rSize = (params.size or 22) * p_scale * click_mul
  local tint = math.max(-0.25, math.min(0.25, vx * 0.012))

  -- 辉光：幽灵轮廓外一层软光。
  local glow_r = params.glowRadius or 15
  if glow_r > 0 then
    ctx:radial_stops(gx, gy + rSize * 0.5, rSize * 1.9 + glow_r, {
      { offset = 0.0,  r = gr, g = gg, b = gb, a = math.floor(ga * 0.30) },
      { offset = 0.55, r = gr, g = gg, b = gb, a = math.floor(ga * 0.10) },
      { offset = 1.0,  r = gr, g = gg, b = gb, a = 0 },
    })
  end

  for i = 1, #dust do
    local p = dust[i]
    ctx:circle(p.x, p.y, math.max(0.5, p.size * p.life), gr, gg, gb,
      math.floor(240 * p.life * 0.6))
  end

  -- 身体 + 手臂
  ctx:polygon(ghost_body(gx, gy, rSize, tint, time), r, g, b, ca)
  ctx:polygon(arm(gx, gy, rSize, tint, -1, math.sin(time * 1.6) * 0.28 - 0.2), r, g, b, ca)
  ctx:polygon(arm(gx, gy, rSize, tint, 1, -math.sin(time * 1.6) * 0.28 + 0.2), r, g, b, ca)

  -- 面部：眼睛朝运动方向看，嘴巴随时间张合。
  local travel = math.sqrt(vx * vx + vy * vy)
  local look_x, look_y = 0, 0
  if travel > 0.5 then
    look_x = vx / travel * 5
    look_y = vy / travel * 3
  end
  local ca_t, sa_t = math.cos(tint), math.sin(tint)
  local function face_pt(x, y)
    return gx + x * ca_t - y * sa_t, gy + x * sa_t + y * ca_t
  end
  local eye_r = math.max(2.0, rSize * 0.12)
  local eye_sp = rSize * 0.38
  local ex, ey = face_pt(-eye_sp + look_x, -rSize * 0.1 + look_y)
  ctx:circle(ex, ey, eye_r, 0, 0, 0, 255)
  ex, ey = face_pt(eye_sp + look_x, -rSize * 0.1 + look_y)
  ctx:circle(ex, ey, eye_r, 0, 0, 0, 255)
  local mouth_w = math.max(2.5, rSize * 0.18)
  local mouth_h = math.max(1.5, rSize * 0.1 + math.max(0, math.sin(time * 2.0) * rSize * 0.05))
  local mx, my = face_pt(look_x, rSize * 0.22 + look_y)
  ctx:ellipse(mx, my, mouth_w, mouth_h, 0, 0, 0, 255)

  -- 指针精确位置：幽灵是弹簧跟随，给一个很小的白点。
  local cursor_xp, cursor_yp = mouse_x or gx, mouse_y or gy
  ctx:circle(cursor_xp, cursor_yp, 2.4, 0, 0, 0, 100)
  ctx:circle(cursor_xp, cursor_yp, 1.6, 255, 255, 255, 220)
end
