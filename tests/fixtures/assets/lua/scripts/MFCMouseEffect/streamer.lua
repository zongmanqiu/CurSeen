-- MFCMouseEffect · Neon Streamer
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 霓虹流光：两层描边（外层宽发光 + 内层窄亮芯），线段越靠头部越粗越亮；
-- 上游默认历史 420ms、headPower 1.6。

name = "Neon Streamer"
effect_name = "Neon Streamer"
kind = "trail"
description = "MFCMouseEffect Neon Streamer：头重尾轻的霓虹流光拖尾。"

local MAX_POINTS = 90
local pts = {}
local last_ms = 0

function schema()
  return {
    { key = "color",       label = "流光颜色", type = "color", alpha = true, default = "#64FFDA" },
    { key = "width",       label = "线宽",     type = "float", min = 1, max = 24, step = 0.5, default = 4 },
    { key = "duration",    label = "历史时长(ms)", type = "int", min = 80, max = 2000, step = 10, default = 420 },
    { key = "head_power",  label = "头重指数", type = "float", min = 0.8, max = 3.0, step = 0.1, default = 1.6 },
    { key = "glow_scale",  label = "辉光倍数", type = "float", min = 0.5, max = 4.0, step = 0.1, default = 1.8 },
    { key = "core_scale",  label = "亮芯倍数", type = "float", min = 0.2, max = 2.0, step = 0.05, default = 0.55 },
    { key = "opacity",     label = "不透明度", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.9 },
  }
end

function init()
  pts = {}
  last_ms = 0
end

function on_move(x, y, t, params)
  -- 逐点强度：上游线宽含速度调制 ResolveLineWidthScale = 1.22×(0.88+0.52·强度)，
  -- 强度 = 速度/24；记录每点生成时的速度供绘制段取用。
  pts[#pts + 1] = {
    x = x, y = y, at = wall_ms or 0,
    inten = clamp((mouse_speed or 0) / 24, 0, 1),
  }
  last_ms = wall_ms or 0
  while #pts > MAX_POINTS do table.remove(pts, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  local dur = params.duration or 420
  while #pts > 0 and now - pts[1].at > dur do table.remove(pts, 1) end
end

function draw(ctx, params)
  local n = #pts
  if n < 2 then return end
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDA")
  local dur = math.max(80, params.duration or 420)
  local width = params.width or 4
  local head_power = clamp(params.head_power or 1.6, 0.8, 3.0)
  local glow_scale = params.glow_scale or 1.8
  local core_scale = params.core_scale or 0.55
  local base_a = (params.opacity or 0.9) * a
  local now = wall_ms or 0
  local width_scale = math.max(0.35, width / 4.0)
  -- 静止加速淡出（同上游 50/260 口径）：停住 260ms 后 50ms 内淡掉。
  local since = now - last_ms
  local idle_fade = 1.0
  if since > 260 then idle_fade = clamp(1 - (since - 260) / 50, 0, 1) end

  for i = 1, n - 1 do
    local p1 = pts[i]
    local p2 = pts[i + 1]
    local ratio = n <= 2 and 1.0 or (i - 1) / (n - 1)
    local life = clamp(1 - (now - p1.at) / dur, 0, 1)
    if life > 0 then
      local head = ratio ^ head_power
      local w = (2.0 + 18.0 * head * life) * width_scale
        * 1.22 * (0.88 + 0.52 * (p1.inten or 0))
      local core_a = math.floor(base_a * idle_fade * clamp(220.0 * head * life / 255.0, 0, 1))
      local glow_a = math.floor(base_a * idle_fade * clamp(90.0 * head * life / 255.0, 0, 1))
      ctx:line(p1.x, p1.y, p2.x, p2.y, w * glow_scale, r, g, b, glow_a)
      ctx:line(p1.x, p1.y, p2.x, p2.y, math.max(1.5, w * core_scale),
        r, g, b, core_a)
    end
  end
end
