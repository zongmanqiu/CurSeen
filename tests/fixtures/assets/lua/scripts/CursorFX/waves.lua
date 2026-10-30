-- CursorFX · waves
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：原版的页面扭曲依赖 Chromium 的 backdrop-filter + SVG 位移图，CurSeen 没有
-- DOM/位图采样，故采用源码自带的回退路径——同心圆环向外扩散并淡出。最多 6 个波。

name = "waves"
effect_name = "waves"
kind = "click"
description = "CursorFX waves：按下点的同心水波环（页面扭曲的回退实现）。"

local MAX_WAVES = 6
local waves = {}

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#C8E6FF" },
    { key = "radius",   label = "最大半径", type = "float", min = 80,  max = 480, step = 10, default = 340 },
    { key = "duration", label = "扩散时长", type = "float", min = 0.6, max = 2.4, step = 0.1, default = 1.4 },
  }
end

function init()
  waves = {}
end

function on_press(x, y, t, params)
  waves[#waves + 1] = { x = x, y = y, age = 0 }
  while #waves > MAX_WAVES do
    table.remove(waves, 1)
  end
end

function step(dt, t, params)
  if #waves == 0 then
    return
  end
  local dur = params.duration or 1.4
  local keep = {}
  for i = 1, #waves do
    local w = waves[i]
    w.age = w.age + dt
    if w.age < dur then
      keep[#keep + 1] = w
    end
  end
  waves = keep
end

function draw(ctx, params)
  if #waves == 0 then
    return
  end
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#C8E6FF")
  local base_a = ca or 255
  local max_r = params.radius or 340
  local dur = params.duration or 1.4
  for i = 1, #waves do
    local w = waves[i]
    local e = clamp(w.age / dur, 0, 1)
    local rad = 24 + max_r * (e ^ 0.6)
    local fade = 1 - e
    local a = math.floor(base_a * fade * 0.7)
    if a > 0 then
      local width = 3 * (1 - e) + 0.5
      ctx:ring(w.x, w.y, rad * 0.72, width, cr, cg, cb, a)
      local ia = math.floor(base_a * fade * 0.28)
      if ia > 0 then
        ctx:ring(w.x, w.y, rad * 0.55, 1, cr, cg, cb, ia)
      end
    end
  end
end
