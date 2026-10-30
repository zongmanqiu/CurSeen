-- MFCMouseEffect · Text
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击漂浮文字：从文字池随机取一条，向上漂浮 60px + 水平漂移 + 正弦摆动 +
-- 0.8→1.2→1.0 缩放。按住 ≥260ms 视为长按，不触发本次点击。
-- 简化：渲染层没有旋转文字，文字保持正立。

name = "Text"
effect_name = "Text"
kind = "click"
description = "MFCMouseEffect Text：点击点漂浮的随机文字。"

local HOLD_SUPPRESS_MS = 260
local MAX_TEXTS = 20
local texts = {}
local press_ms = 0

function schema()
  return {
    { key = "texts",          label = "文字池", type = "str", default = "美丽,健康,幸福,财富+1w,开心" },
    { key = "colors",         label = "颜色池(逗号分隔)", type = "str", default = "#FF69B4,#87CEEB,#FFD700,#32CD32,#BA55D3" },
    { key = "font_size",      label = "字号",   type = "int",   min = 8,   max = 64,  step = 1,   default = 11 },
    { key = "float_distance", label = "上浮距离", type = "int", min = 0,   max = 240, step = 5,   default = 60 },
    { key = "drift",          label = "水平漂移", type = "int", min = 0,   max = 120, step = 5,   default = 50 },
    { key = "duration",       label = "时长(ms)", type = "int", min = 100, max = 3000,step = 20,  default = 800 },
  }
end

local function split_pool(s)
  local out = {}
  for item in string.gmatch(s or "", "([^,]+)") do
    local v = item:gsub("^%s+", ""):gsub("%s+$", "")
    if v ~= "" then out[#out + 1] = v end
  end
  if #out == 0 then out[1] = "Click" end
  return out
end

function init()
  texts = {}
  press_ms = 0
end

function on_mouse_down(button, x, y, params)
  press_ms = wall_ms or 0
end

function on_mouse_up(button, x, y, params)
  if (wall_ms or 0) - press_ms >= HOLD_SUPPRESS_MS then return end
  params = params or {}
  local pool = split_pool(params.texts or "美丽,健康,幸福,财富+1w,开心")
  -- 上游每次点击从 5 色池随机取色（TextConfig.colors）；旧版固定单色。
  local cpool = split_pool(params.colors or "#FF69B4,#87CEEB,#FFD700,#32CD32,#BA55D3")
  local r, g, b, a = hex_to_rgb(cpool[rng_int(1, #cpool)])
  local drift = params.drift or 50
  texts[#texts + 1] = {
    x = x, y = y,
    start = wall_ms or 0,
    duration = math.max(100, params.duration or 800),
    text = pool[rng_int(1, #pool)],
    size = params.font_size or 11,
    float = params.float_distance or 60,
    drift = rng(-drift, drift),
    sway = 1.0 + rng(0, 1.99),
    sway_amp = 5.0 + rng(0, 9.9),
    r = r, g = g, b = b, a = a,
  }
  while #texts > MAX_TEXTS do table.remove(texts, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  for i = #texts, 1, -1 do
    if now - texts[i].start >= texts[i].duration then table.remove(texts, i) end
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  for i = 1, #texts do
    local p = texts[i]
    local t = clamp((now - p.start) / p.duration, 0, 1)
    local eased = 1 - (1 - t) * (1 - t) * (1 - t)
    local alpha
    if t < 0.15 then
      alpha = t / 0.15
    elseif t > 0.6 then
      alpha = clamp(1 - (t - 0.6) / 0.4, 0, 1)
    else
      alpha = 1
    end
    local scale
    if t < 0.3 then
      scale = 0.8 + (t / 0.3) * 0.4
    else
      scale = 1.2 - ((t - 0.3) / 0.7) * 0.2
    end
    local x = p.x + t * p.drift + math.sin(t * math.pi * p.sway) * p.sway_amp
    local y = p.y - eased * p.float
    ctx:text_centered(x, y, p.text, p.size * scale, p.r, p.g, p.b,
      math.floor(p.a * alpha))
  end
end
