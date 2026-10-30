-- CursorFX · ribbon / 复刻自 cursorfx-main/src/cursors/ribbon.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：一条绸带跟在指针后面，越靠尾越窄，宽度还被正弦“扭转”调制；颜色沿长度
-- 在五色间循环并随时间流动。头部是一颗白点。
--
-- 实现：40 个滞后采样点，相邻点之间填一个四边形（不是整条路径），宽度按段计算。
-- 每帧 39 个 4 点多边形，约 156 个顶点；SourceOver 普通混合，成本很低。

name = "ribbon"
effect_name = "ribbon"
kind = "cursor"
description = "CursorFX ribbon：五色流光绸带拖尾。"
hide_cursor = true

local PALETTE = {
  { 255, 107, 107 },
  { 255, 217, 61 },
  { 107, 203, 119 },
  { 77, 150, 255 },
  { 199, 125, 255 },
}

function schema()
  return {
    { key = "length",  label = "长度",   type = "int",   min = 8,  max = 80, step = 1,  default = 40 },
    { key = "width",   label = "带宽",   type = "int",   min = 4,  max = 40, step = 1,  default = 16 },
    { key = "rainbow", label = "五色流动", type = "bool", default = true },
    { key = "color",   label = "单色",   type = "color", alpha = true, default = "#FF6B6B" },
  }
end

local xs, ys = {}, {}
local count = 0

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  xs, ys = {}, {}
  count = 0
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  local target = math.floor(clampf(params.length or 40, 8, 80))

  if count == 0 then
    for i = 1, target do
      xs[i], ys[i] = mx, my
    end
    count = target
  elseif target > count then
    for i = count + 1, target do
      xs[i], ys[i] = xs[count], ys[count]
    end
    count = target
  elseif target < count then
    for i = target + 1, count do
      xs[i], ys[i] = nil, nil
    end
    count = target
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k0 = math.min(1.0, 0.7 * f)
  xs[1] = xs[1] + (mx - xs[1]) * k0
  ys[1] = ys[1] + (my - ys[1]) * k0
  local k = math.min(1.0, 0.5 * f)
  for i = 2, count do
    xs[i] = xs[i] + (xs[i - 1] - xs[i]) * k
    ys[i] = ys[i] + (ys[i - 1] - ys[i]) * k
  end
end

local function width_at(base, t, time, i)
  return base * (1.0 - t) * (0.55 + 0.45 * math.abs(math.sin(time * 5.0 + i * 0.35)))
end

function draw(ctx, params)
  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  -- ★ 放在 count 守卫之前：静止时绸带收敛成零面积、首帧 step 未跑时 count=0，
  --   这两种状态下唯一能标出指针位置的就是这颗点，不能被早退守卫跳过。
  local pmx, pmy = mouse_x or xs[1] or 0, mouse_y or ys[1] or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

  if count < 2 then return end

  local time = elapsed or 0
  local base_w = params.width or 16
  local rainbow = params.rainbow
  if rainbow == nil then rainbow = true end
  local fr, fg, fb, fa = hex_to_rgb(params.color or "#FF6B6B")

  for i = 1, count - 1 do
    local t = (i - 1) / count
    local w = width_at(base_w, t, time, i)
    local dx = xs[i + 1] - xs[i]
    local dy = ys[i + 1] - ys[i]
    local len = math.sqrt(dx * dx + dy * dy)
    if len < 0.001 then len = 0.001 end
    local nx = -dy / len * w * 0.5
    local ny = dx / len * w * 0.5

    local w2 = width_at(base_w, i / count, time, i + 1)
    local dx2, dy2
    if i + 2 <= count then
      dx2 = xs[i + 2] - xs[i + 1]
      dy2 = ys[i + 2] - ys[i + 1]
    else
      dx2, dy2 = dx, dy
    end
    local len2 = math.sqrt(dx2 * dx2 + dy2 * dy2)
    if len2 < 0.001 then len2 = 0.001 end
    local nx2 = -dy2 / len2 * w2 * 0.5
    local ny2 = dx2 / len2 * w2 * 0.5

    local cr, cg, cb
    if rainbow then
      local idx = math.floor(t * #PALETTE + time * 2.0) % #PALETTE + 1
      local col = PALETTE[idx]
      cr, cg, cb = col[1], col[2], col[3]
    else
      cr, cg, cb = fr, fg, fb
    end

    local alpha = math.floor(255.0 * 0.9 * (1.0 - t) * (fa / 255.0))

    ctx:polygon({
      { xs[i] + nx,      ys[i] + ny },
      { xs[i + 1] + nx2, ys[i + 1] + ny2 },
      { xs[i + 1] - nx2, ys[i + 1] - ny2 },
      { xs[i] - nx,      ys[i] - ny },
    }, cr, cg, cb, alpha)
  end
end
