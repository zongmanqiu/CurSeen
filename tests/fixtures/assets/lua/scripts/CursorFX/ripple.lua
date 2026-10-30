-- CursorFX · ripple
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：按下点生成若干同心水环，向外扩散并淡出。用 on_press 写，复制到
-- ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的 on_press 兼容分支驱动。

name = "ripple"
effect_name = "ripple"
kind = "click"
description = "CursorFX ripple：按下点的同心水环。"

local rings = {}

function schema()
  return {
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#C8E6FF" },
    { key = "radius",  label = "最大半径", type = "int",   min = 16, max = 320, step = 1, default = 120 },
    { key = "duration",label = "扩散时长", type = "float", min = 0.15, max = 2.0, step = 0.05, default = 1.0 },
    { key = "count",   label = "环数",     type = "int",   min = 1,  max = 6,   step = 1, default = 3 },
  }
end

function init()
  rings = {}
end

function on_press(x, y, t, params)
  local count = math.floor(params.count or 3)
  local dur = params.duration or 1.0
  for i = 1, count do
    rings[#rings + 1] = {
      x = x,
      y = y,
      born = t or elapsed or 0,
      delay = (i - 1) * 0.1,
      -- 原版后环寿命更长（ripple.js：60+12i 帧，i 从 0 起）——首环 1.0x，
      -- 后面每环多 0.2x，这样后环不会先于前环消失。
      life = dur * (1.0 + 0.2 * (i - 1)),
    }
  end
end

function step(dt, t, params)
  local keep = {}
  for i = 1, #rings do
    local r = rings[i]
    if (t or 0) - r.born < r.life + r.delay then
      keep[#keep + 1] = r
    end
  end
  rings = keep
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#C8E6FF")
  local max_r = params.radius or 120
  local now = elapsed or 0
  for i = 1, #rings do
    local ring = rings[i]
    local age = now - ring.born - ring.delay
    if age >= 0 and age < ring.life then
      local p = age / ring.life
      -- 扩散曲线同原版 t^0.6（先快后慢）；每波双描：彩环 + 0.8r 白色内环
      -- （ripple.js:27），旧实现只有单环且用了 1-(1-p)² 曲线。
      local radius = max_r * math.pow(p, 0.6)
      local alpha = math.floor((a or 255) * (1.0 - p))
      ctx:ring(ring.x, ring.y, radius, 1.5, r, g, b, alpha)
      ctx:ring(ring.x, ring.y, radius * 0.8, 1.5, 255, 255, 255, alpha)
    end
  end
end
