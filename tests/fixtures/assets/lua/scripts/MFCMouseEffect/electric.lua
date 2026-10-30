-- MFCMouseEffect · Cyber Electric
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 赛博电弧：每段折线拆成 3 段，沿法线随机抖动；外层青蓝辉光 + 白色亮芯，
-- 长段偶发分叉。上游默认历史 280ms、amplitude 1.0、forkChance 0.10。

name = "Cyber Electric"
effect_name = "Cyber Electric"
kind = "trail"
description = "MFCMouseEffect Cyber Electric：抖动分叉的赛博电弧拖尾。"

local MAX_POINTS = 48
local pts = {}

function schema()
  return {
    { key = "color",        label = "电弧颜色", type = "color", alpha = true, default = "#64FFDA" },
    { key = "width",        label = "线宽",     type = "float", min = 1, max = 12, step = 0.2, default = 2.2 },
    { key = "duration",     label = "历史时长(ms)", type = "int", min = 80, max = 1500, step = 10, default = 280 },
    { key = "amplitude",    label = "抖动幅度", type = "float", min = 0.2, max = 3.0, step = 0.1, default = 1.0 },
    { key = "fork_chance",  label = "分叉概率", type = "float", min = 0, max = 0.5, step = 0.01, default = 0.10 },
    { key = "opacity",      label = "不透明度", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.9 },
  }
end

function init()
  pts = {}
end

function on_move(x, y, t, params)
  -- 逐点速度强度（上游核心宽 3.3–5.3 随速度，速度/24 口径同 streamer）。
  pts[#pts + 1] = {
    x = x, y = y, at = wall_ms or 0,
    inten = clamp((mouse_speed or 0) / 24, 0, 1),
  }
  while #pts > MAX_POINTS do table.remove(pts, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  local dur = params.duration or 280
  while #pts > 0 and now - pts[1].at > dur do table.remove(pts, 1) end
end

function draw(ctx, params)
  local n = #pts
  if n < 2 then return end
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDA")
  local dur = math.max(80, params.duration or 280)
  local width = params.width or 2.2
  local amp_scale = clamp(params.amplitude or 1.0, 0.2, 3.0)
  local fork_chance = clamp(params.fork_chance or 0.10, 0, 0.5)
  local base_a = (params.opacity or 0.9) * a
  local now = wall_ms or 0

  for i = 1, n - 1 do
    local p1 = pts[i]
    local p2 = pts[i + 1]
    local life = clamp(1 - (now - p1.at) / dur, 0, 1)
    if life > 0 then
      local dx, dy = p2.x - p1.x, p2.y - p1.y
      local len = math.sqrt(dx * dx + dy * dy)
      if len >= 0.5 then
        local nx, ny = -dy / len, dx / len
        local amp = clamp(len * 0.12, 2.0, 10.0) * life * amp_scale
        local o1 = rng(-1, 1) * amp
        local o2 = rng(-1, 1) * amp
        local ax, ay = p1.x, p1.y
        local bx, by = p1.x + dx * 0.35 + nx * o1, p1.y + dy * 0.35 + ny * o1
        local cx, cy = p1.x + dx * 0.70 + nx * o2, p1.y + dy * 0.70 + ny * o2
        local dx2, dy2 = p2.x, p2.y
        local core_w = math.max(1.0, 3.3 + 2.0 * (p1.inten or 0))
        -- 辉光宽随 life 收缩（上游 glowWidthPx = max(1, 宽·life)×3），旧版恒定。
        local glow_w = math.max(1.0, width * life) * 3.0
        local core_a = math.floor(base_a * life)
        local glow_a = math.floor(base_a * life * 0.5)
        -- 辉光偏青绿，亮芯纯白（上游的“电弧”识别度）。
        local gr = math.max(70, r)
        local gg = math.max(200, g)
        local gb = math.max(220, b)
        local segs = { { ax, ay, bx, by }, { bx, by, cx, cy }, { cx, cy, dx2, dy2 } }
        for k = 1, 3 do
          local s = segs[k]
          ctx:line(s[1], s[2], s[3], s[4], glow_w, gr, gg, gb, glow_a)
        end
        for k = 1, 3 do
          local s = segs[k]
          ctx:line(s[1], s[2], s[3], s[4], core_w, 255, 255, 255, core_a)
        end
        local fork_len_factor = clamp((len - 4.0) / 20.0, 0, 1)
        if fork_len_factor > 0.05 and rng(0, 1) < fork_chance * life *
          (0.2 + 0.8 * fork_len_factor) then
          local tpos = rng(0.35, 0.75)
          local bx2 = p1.x + dx * tpos
          local by2 = p1.y + dy * tpos
          local side = rng(0, 1) < 0.5 and -1 or 1
          local flen = rng(10, 22) * life * clamp(0.25 + 0.75 * fork_len_factor, 0.25, 1.0)
          local tipx = bx2 + nx * side * flen + dx * 0.05
          local tipy = by2 + ny * side * flen + dy * 0.05
          ctx:line(bx2, by2, tipx, tipy, math.max(1.0, core_w * 1.2),
            gr, gg, gb, glow_a)
        end
      end
    end
  end
end
