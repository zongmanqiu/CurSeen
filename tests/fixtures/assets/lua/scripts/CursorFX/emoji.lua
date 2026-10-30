-- CursorFX · emoji / 复刻自 cursorfx-main/src/cursors/emoji.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：表情用弹簧挂在指针上，移动时会冲过头再弹回；按下整体缩小。
-- CurSeen 没有字体旋转 / 图片旋转，也没有任意图片加载：这里用 text_centered 正立
-- 渲染表情，倾斜与沿运动方向的拉伸省略，改用地面椭圆影子 + 尾随残影表达弹性。
--
-- 性能：每帧 1~3 次文字绘制 + 1 个影子椭圆，恒定开销。

name = "emoji"
effect_name = "emoji"
kind = "cursor"
description = "CursorFX emoji：弹簧跟随的表情光标，可换多种表情。"
hide_cursor = true

function schema()
  return {
    { key = "emoji", label = "表情", type = "choice",
      choices = { "🐱", "🐶", "🐸", "🦊", "🐼", "🌟", "👾", "🍕" }, default = "🐱" },
    { key = "size",      label = "大小",   type = "int",   min = 16, max = 80, step = 1,    default = 36 },
    { key = "stiffness", label = "弹性",   type = "float", min = 0.04, max = 0.30, step = 0.01, default = 0.12 },
    { key = "damping",   label = "阻尼",   type = "float", min = 0.50, max = 0.95, step = 0.01, default = 0.78 },
    { key = "backing",   label = "暗底",   type = "color", alpha = true, default = "#000000B8" },
    { key = "outline",   label = "外圈颜色", type = "color", default = "#FFFFFF" },
    { key = "rainbow",   label = "彩虹外圈", type = "bool", default = true },
    { key = "color",     label = "文字颜色", type = "color", default = "#FFFFFF" },
  }
end

local p = { x = 0, y = 0, vx = 0, vy = 0 }
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  p.x, p.y, p.vx, p.vy = 0, 0, 0, 0
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    p.x, p.y = mx, my
    started = true
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local stiffness = params.stiffness or 0.12
  local damping = params.damping or 0.78

  p.vx = p.vx + (mx - p.x) * stiffness * f
  p.vy = p.vy + (my - p.y) * stiffness * f
  local d = damping ^ f
  p.vx = p.vx * d
  p.vy = p.vy * d
  p.x = p.x + p.vx * f
  p.y = p.y + p.vy * f
end

function draw(ctx, params)
  if not started then return end

  local e = params.emoji or "🐱"
  local size = params.size or 36
  if mouse_down then size = size * 0.85 end

  local sp = math.sqrt(p.vx * p.vx + p.vy * p.vy)
  local squash = 1.0 + clampf(sp * 0.02, 0.0, 0.35)

  -- 地面影子：弹簧拉伸时横向变宽、纵向变扁，作为“果冻感”的替代。
  local shadow_y = p.y + size * 0.42
  ctx:ellipse(p.x, shadow_y, size * 0.30 * squash, size * 0.075 / math.sqrt(squash), 0, 0, 0, 52)

  -- 暗底 + 彩色外圈：表情字形在浅色/彩色背景上也能被定位。
  local br, bg, bb, ba = hex_to_rgb(params.backing or "#000000B8")
  ctx:circle(p.x, p.y, size * 0.64, br, bg, bb, ba)
  local orr, ogg, obb, oa = hex_to_rgb(params.outline or "#FFFFFF")
  local or_, og, ob
  if params.rainbow ~= false then
    or_, og, ob = hsv_to_rgb(((elapsed or 0) * 70.0 + size * 2.0) % 360.0, 1.0, 1.0)
    oa = math.floor(230 * oa / 255)
  else
    or_, og, ob = orr, ogg, obb
  end
  ctx:ring(p.x, p.y, size * 0.68, 2.0, or_, og, ob, oa)

  -- 快速移动时留两道很淡的残影（正立、不旋转）。
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFFFFF")
  if sp > 2.0 then
    local a = math.floor(clampf(sp * 9.0, 0.0, 54.0) * ca / 255)
    ctx:text_centered(p.x - p.vx * 0.55, p.y - p.vy * 0.55, e, size * 0.92, cr, cg, cb, a)
    ctx:text_centered(p.x - p.vx * 0.28, p.y - p.vy * 0.28, e, size * 0.96, cr, cg, cb, math.floor(a * 1.6))
  end

  ctx:text_centered(p.x, p.y, e, size, cr, cg, cb, ca)
end
