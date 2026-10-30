-- CursorFX · ring
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：隐藏系统指针，在指针处画“内点 + 滞后外环”。环半径随状态变化
-- （同原版 ring.js:15）：按下收縮 0.6x、悬停交互提示时放大 2.2x、平时随速度
-- 膨胀。原版还用 difference 反色混合画环（ring.js:22）——CurSeen 的 ctx 未开放
-- 混合模式，此处以普通 SourceOver 代替，属已登记的宿主能力差异。

name = "ring"
effect_name = "ring"
kind = "cursor"
description = "CursorFX ring：点 + 滞后圆环光标。"
hide_cursor = true

function schema()
  return {
    { key = "ring_color", label = "外环颜色", type = "color", alpha = true, default = "#FFFFFF" },
    { key = "dot_color",  label = "中心颜色", type = "color", alpha = true, default = "#FF5252" },
    { key = "dot_outline",label = "中心描边", type = "color", alpha = true, default = "#000000" },
    { key = "size",      label = "外环半径", type = "int",   min = 8,  max = 80,  step = 1, default = 18 },
    { key = "dot",       label = "中心点",   type = "int",   min = 1,  max = 12,  step = 1, default = 4 },
    { key = "thickness", label = "环宽",     type = "int",   min = 1,  max = 6,   step = 1, default = 2 },
    { key = "lag",       label = "滞后",     type = "float", min = 0,  max = 0.9, step = 0.01, default = 0.22 },
  }
end

local rx, ry = 0, 0

function init()
  rx, ry = mouse_x or 0, mouse_y or 0
end

function on_move(x, y, t, params)
  local k = 1.0 - (params.lag or 0.22)
  rx = rx + (x - rx) * k
  ry = ry + (y - ry) * k
end

function step(dt, t, params)
  -- 鼠标不动时 on_move 不触发，环仍缓慢吸附到当前指针，避免切屏后停在旧位置。
  local k = math.min(1.0, dt * 14.0)
  rx = rx + ((mouse_x or rx) - rx) * k
  ry = ry + ((mouse_y or ry) - ry) * k
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.ring_color or "#FFFFFF")
  local dr, dg, db, da = hex_to_rgb(params.dot_color or "#FF5252")
  local orr, org, orb, ora = hex_to_rgb(params.dot_outline or "#000000")
  local size = params.size or 18
  local dot = params.dot or 4
  local thick = params.thickness or 2
  -- 环半径随状态变化（ring.js:15）：按下 0.6x / 悬停 2.2x / 平时随速度膨胀。
  -- 换成原版语义前的旧实现是 ±4% 时间脉冲，已删。
  local radius
  if mouse_down then
    radius = size * 0.6
  elseif cursor_hover then
    radius = size * 2.2
  else
    radius = size + (mouse_speed or 0) * 0.4
  end

  ctx:ring(rx, ry, radius, thick, r, g, b, a)
  -- 中心点先黑描边再填主题色：白墙、白窗口、浅色背景上都能定位指针。
  ctx:circle(mouse_x or rx, mouse_y or ry, dot + 1.5, orr, org, orb, ora)
  ctx:circle(mouse_x or rx, mouse_y or ry, dot, dr, dg, db, da)
end
