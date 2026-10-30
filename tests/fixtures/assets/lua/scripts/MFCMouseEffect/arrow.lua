-- MFCMouseEffect · Arrow
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 方向指示：滚轮时沿滚动轴叠放 3 个 V 形箭头，双层发光 + 实心描边，三次方缓出
-- 向外漂移并淡出；方向跟随纵向/横向滚轮。上游默认时长 230ms。

name = "Arrow"
effect_name = "Arrow"
kind = "scroll"
description = "MFCMouseEffect Arrow：跟随滚轮方向的三层 V 形箭头。"

local MAX_PULSES = 8
local pulses = {}

function schema()
  return {
    { key = "color",    label = "箭头颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "glow",     label = "辉光颜色", type = "color", alpha = true, default = "#FF5AF766" },
    { key = "start",    label = "起始半径", type = "float", min = 1,  max = 40,  step = 1,   default = 8 },
    { key = "size",     label = "结束半径", type = "float", min = 20, max = 120, step = 2,   default = 58 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 8,   step = 0.1, default = 2.8 },
    { key = "duration", label = "时长(ms)", type = "int",   min = 80, max = 800, step = 10,  default = 380 },
  }
end

function init()
  pulses = {}
end

function on_wheel(delta, horizontal, x, y, params)
  if delta == 0 then return end
  params = params or {}
  local dir
  if horizontal then
    dir = delta >= 0 and 0 or math.pi
  else
    dir = delta >= 0 and -math.pi / 2 or math.pi / 2
  end
  local r, g, b, a = hex_to_rgb(params.color or "#FF5AF7")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#FF5AF766")
  pulses[#pulses + 1] = {
    x = x, y = y,
    start = wall_ms or 0,
    duration = math.max(80, params.duration or 380),
    start_r = params.start or 8,
    end_r = params.size or 58,
    width = params.width or 2.8,
    dir = dir,
    r = r, g = g, b = b, a = a,
    gr = gr, gg = gg, gb = gb, ga = ga,
  }
  while #pulses > MAX_PULSES do table.remove(pulses, 1) end
end

function step(dt, t, params)
  local now = wall_ms or 0
  for i = #pulses, 1, -1 do
    if now - pulses[i].start >= pulses[i].duration then table.remove(pulses, i) end
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  for i = 1, #pulses do
    local p = pulses[i]
    local t = clamp((now - p.start) / p.duration, 0, 1)
    local eased = 1 - (1 - t) * (1 - t) * (1 - t)
    local alpha = (1 - eased) * p.a / 255
    if alpha > 0.01 then
      local radius = p.start_r + (p.end_r - p.start_r) * eased
      local dx, dy = math.cos(p.dir), math.sin(p.dir)
      local px, py = -dy, dx
      local drift = radius * 0.10 * (1 - eased)
      for k = 0, 2 do
        local offset = k * (radius * 0.26) + drift
        local length = radius * 1.12
        local half = p.width * (3.1 - k * 0.65)
        local tipx = p.x + dx * (length * 0.5 - offset)
        local tipy = p.y + dy * (length * 0.5 - offset)
        local tailx = p.x - dx * (length * 0.5 + offset)
        local taily = p.y - dy * (length * 0.5 + offset)
        local lx, ly = tailx + px * half, taily + py * half
        local rx, ry = tailx - px * half, taily - py * half
        local fade = 1 - k * 0.20
        local aa = math.floor(255 * alpha * fade)
        local ga = math.floor(p.ga * alpha * fade)
        ctx:line(tipx, tipy, lx, ly, p.width + 9.0, p.gr, p.gg, p.gb,
          math.floor(ga * 0.55))
        ctx:line(tipx, tipy, rx, ry, p.width + 9.0, p.gr, p.gg, p.gb,
          math.floor(ga * 0.55))
        ctx:line(tipx, tipy, lx, ly, p.width + 5.5, p.gr, p.gg, p.gb, ga)
        ctx:line(tipx, tipy, rx, ry, p.width + 5.5, p.gr, p.gg, p.gb, ga)
        ctx:line(tipx, tipy, lx, ly, p.width + 1.0, p.r, p.g, p.b, aa)
        ctx:line(tipx, tipy, rx, ry, p.width + 1.0, p.r, p.g, p.b, aa)
      end
    end
  end
end
