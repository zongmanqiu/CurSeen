-- MFCMouseEffect · Line
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 普通线条：历史折线按年龄淡出，圆头圆角。上游默认线宽 4、历史 300ms。

name = "Line"
effect_name = "Line"
kind = "trail"
description = "MFCMouseEffect Line：按年龄淡出的普通折线拖尾。"

local MAX_POINTS = 64
local pts = {}
local last_ms = 0

function schema()
  return {
    { key = "color",    label = "线条颜色", type = "color", alpha = true, default = "#64FFDA" },
    { key = "width",    label = "线宽",     type = "float", min = 1, max = 24, step = 0.5, default = 4 },
    { key = "duration", label = "历史时长(ms)", type = "int", min = 80, max = 2000, step = 10, default = 300 },
    { key = "opacity",  label = "不透明度", type = "float", min = 0.05, max = 1, step = 0.01, default = 1.0 },
  }
end

function init()
  pts = {}
  last_ms = 0
end

function on_move(x, y, t, params)
  pts[#pts + 1] = { x = x, y = y, at = wall_ms or 0 }
  last_ms = wall_ms or 0
  while #pts > MAX_POINTS do table.remove(pts, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  local dur = params.duration or 300
  while #pts > 0 and now - pts[1].at > dur do table.remove(pts, 1) end
end

function draw(ctx, params)
  if #pts < 2 then return end
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDA")
  local dur = math.max(80, params.duration or 300)
  local width = params.width or 4
  local now = wall_ms or 0
  local base_a = (params.opacity or 1.0) * a
  -- 静止加速淡出（上游 LineTrailRenderer.h idleFade 60/220）：停住 220ms 后
  -- 60ms 内淡掉；上游出生 alpha 即 life×255（不另乘 0.86）。
  local since = now - last_ms
  local idle_fade = 1.0
  if since > 220 then idle_fade = clamp(1 - (since - 220) / 60, 0, 1) end

  for i = 1, #pts - 1 do
    local p1 = pts[i]
    local p2 = pts[i + 1]
    local life = clamp(1 - (now - p1.at) / dur, 0, 1)
    if life > 0 then
      ctx:line(p1.x, p1.y, p2.x, p2.y, width,
        r, g, b, math.floor(base_a * idle_fade * life))
    end
  end
end
