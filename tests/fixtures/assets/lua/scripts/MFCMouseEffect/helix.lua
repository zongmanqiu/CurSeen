-- MFCMouseEffect · 3D Helix
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 3D 双螺旋：沿滚动轴拉出两条缠绕的螺旋带，逐段按伪 3D 透视排序后绘制，
-- 外层柔光 + 内层亮线 + 双头高光。上游 44 段/条、默认时长 240ms。

name = "3D Helix"
effect_name = "3D Helix"
kind = "scroll"
description = "MFCMouseEffect 3D Helix：沿滚轮方向展开的 3D 双螺旋。"

local MAX_PULSES = 6
local pulses = {}
local SEGMENTS = 44

function schema()
  return {
    { key = "color",    label = "链一颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "glow",     label = "链二颜色", type = "color", alpha = true, default = "#FF5AF7" },
    { key = "start",    label = "起始半径", type = "float", min = 1,  max = 40,  step = 1,   default = 8 },
    { key = "size",     label = "结束半径", type = "float", min = 20, max = 140, step = 2,   default = 58 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 8,   step = 0.1, default = 2.8 },
    { key = "duration", label = "时长(ms)", type = "int",   min = 80, max = 800, step = 10,  default = 400 },
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
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FF5AF7")
  local gr, gg, gb, ga = hex_to_rgb(params.glow or "#FF5AF7")
  pulses[#pulses + 1] = {
    x = x, y = y,
    start = wall_ms or 0,
    duration = math.max(80, params.duration or 400),
    start_r = params.start or 8,
    end_r = params.size or 58,
    width = params.width or 2.8,
    dir = dir,
    cr = cr, cg = cg, cb = cb, ca = ca,
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

local function smoothstep(a, b, x)
  if b <= a then return x >= b and 1 or 0 end
  local t = clamp((x - a) / (b - a), 0, 1)
  return t * t * (3 - 2 * t)
end

local function mix(a, b, t) return a * (1 - t) + b * t end

local function helix_pulse(ctx, p, t)
  local tn = clamp(t, 0, 1)
  local fade = 1 - smoothstep(0.72, 1, tn)
  if fade <= 0.005 then return end
  local dir = p.dir + math.pi
  local dx, dy = math.cos(dir), math.sin(dir)
  local px, py = -dy, dx
  local seconds = tn * p.duration / 1000.0
  local phase = seconds * 1.55 * math.pi
  local length = (p.end_r - p.start_r) * 2.05
  local radius = p.width * 2.2 + 6.2
  local camera = radius * 5.2
  local depth_amp = radius * 0.95
  local stroke = { p.cr / 255, p.cg / 255, p.cb / 255 }
  local glow = { p.gr / 255, p.gg / 255, p.gb / 255 }

  local function point(i, offset)
    local u = i / SEGMENTS
    local dist = (u - 0.5) * length + (1 - fade) * (length * 0.14)
    local angle = u * 1.72 * 2 * math.pi - phase + offset
    local ly = math.sin(angle) * radius
    local lz = math.cos(angle) * depth_amp
    local persp = camera / (camera + lz + depth_amp)
    local sx = p.x + dx * dist + px * (ly * persp)
    local sy = p.y + dy * dist + py * (ly * persp)
    local taper = 0.30 + 0.70 * u
    local depth_light = 0.52 + 0.48 * ((lz / depth_amp) * 0.5 + 0.5)
    local head_boost = 0.62 + 0.38 * smoothstep(0.55, 1, u)
    return {
      x = sx, y = sy, z = lz / depth_amp,
      w = (0.62 + 0.38 * persp) * taper,
      a = taper * depth_light * head_boost,
      u = u,
    }
  end

  local segs = {}
  for strand = 0, 1 do
    local offset = strand * math.pi
    local col = strand == 0 and stroke or glow
    for i = 0, SEGMENTS - 1 do
      local p1 = point(i, offset)
      local p2 = point(i + 1, offset)
      local avg_z = (p1.z + p2.z) * 0.5
      local avg_w = (p1.w + p2.w) * 0.5 * (p.width + 0.9)
      local strand_a = ((strand == 0) and p.ca or p.ga) / 255
      local avg_a = (p1.a + p2.a) * 0.5 * fade * strand_a
      local avg_u = (p1.u + p2.u) * 0.5
      local sm = strand == 0 and (0.25 + 0.45 * avg_u) or (0.20 + 0.32 * avg_u)
      segs[#segs + 1] = {
        z = avg_z, x1 = p1.x, y1 = p1.y, x2 = p2.x, y2 = p2.y,
        w = avg_w, a = avg_a,
        r = mix(col[1], 1, sm), g = mix(col[2], 1, sm), b = mix(col[3], 1, sm),
      }
    end
  end
  table.sort(segs, function(a, b) return a.z < b.z end)
  for i = 1, #segs do
    local s = segs[i]
    local aa = math.floor(s.a * 255)
    if aa > 0 then
      ctx:line(s.x1, s.y1, s.x2, s.y2, math.max(1.0, s.w + 2.1),
        math.floor(s.r * 255), math.floor(s.g * 255), math.floor(s.b * 255),
        math.floor(aa * 0.24))
      ctx:line(s.x1, s.y1, s.x2, s.y2, math.max(1.0, s.w),
        math.floor(s.r * 255), math.floor(s.g * 255), math.floor(s.b * 255), aa)
    end
  end

  for strand = 0, 1 do
    local offset = strand * math.pi
    local h = point(SEGMENTS, offset)
    local head_a = math.floor(fade * 255 * (((strand == 0) and p.ca or p.ga) / 255))
    local rr = math.max(1.3, p.width * 0.95)
    ctx:circle(h.x, h.y, rr, 255, 255, 255, math.floor(head_a * 0.82))
    ctx:circle(h.x, h.y, rr * 1.85, p.cr, p.cg, p.cb, math.floor(head_a * 0.18))
  end
end

function draw(ctx, params)
  local now = wall_ms or 0
  for i = 1, #pulses do
    local p = pulses[i]
    helix_pulse(ctx, p, (now - p.start) / p.duration)
  end
end
