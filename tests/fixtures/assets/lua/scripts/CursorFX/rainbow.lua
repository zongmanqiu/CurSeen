-- CursorFX · rainbow
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：一条七色带跟随指针，头节点快速吸附、尾节点依次滞后，越靠尾越细越淡。
-- 颜色逐段用 HSV 现算（相邻色带相位错开），而不是写死七组 RGB。

name = "rainbow"
effect_name = "rainbow"
kind = "trail"
description = "CursorFX rainbow：七色 HSV 彩虹缎带拖尾。"

local MAX_POINTS = 64
local DEFAULT_POINTS = 45
local BAND_HUE = { 0, 30, 60, 142, 190, 240, 285 }

local points = {}
local seeded = false

function schema()
  return {
    { key = "length",    label = "节点数",   type = "int",   min = 12, max = 64,  step = 1,    default = 45 },
    { key = "width",     label = "带宽",     type = "float", min = 6,  max = 44,  step = 1,    default = 22 },
    { key = "hue_speed", label = "色相转速", type = "float", min = 0,  max = 120, step = 5,    default = 0 },
    { key = "opacity",   label = "不透明度", type = "float", min = 0.1, max = 1,  step = 0.05, default = 0.9 },
  }
end

local function ensure_points(n, x, y)
  while #points < n do
    local last = points[#points]
    points[#points + 1] = {
      x = last and last.x or x,
      y = last and last.y or y,
    }
  end
  while #points > n do
    table.remove(points)
  end
end

local function seed(x, y, n)
  points = {}
  ensure_points(n, x, y)
  seeded = true
end

function init()
  points = {}
  seeded = false
end

function on_move(x, y, t, params)
  if not seeded then
    local n = math.floor(clamp(params.length or DEFAULT_POINTS, 12, MAX_POINTS))
    seed(x, y, n)
  end
end

function step(dt, t, params)
  local n = math.floor(clamp(params.length or DEFAULT_POINTS, 12, MAX_POINTS))
  if not seeded then
    seed(mouse_x or 0, mouse_y or 0, n)
  else
    ensure_points(n, mouse_x or 0, mouse_y or 0)
  end
  if #points < 2 then return end

  -- 与 60fps 原版等价：每 1/60 秒向目标靠拢 80%（头）/ 55%（尾）。
  local frames = dt * 60
  local kh = 1 - (1 - 0.8) ^ frames
  local kt = 1 - (1 - 0.55) ^ frames

  local head = points[1]
  head.x = head.x + ((mouse_x or head.x) - head.x) * kh
  head.y = head.y + ((mouse_y or head.y) - head.y) * kh

  for i = 2, #points do
    local p = points[i]
    local prev = points[i - 1]
    p.x = p.x + (prev.x - p.x) * kt
    p.y = p.y + (prev.y - p.y) * kt
  end
end

function draw(ctx, params)
  if #points < 2 then return end

  local now = elapsed or 0
  local n = #points
  local width = params.width or 22
  local bw = width / #BAND_HUE
  local alpha0 = params.opacity or 0.9
  local hue_speed = params.hue_speed or 0

  for i = 1, n - 1 do
    local a = points[i]
    local b = points[i + 1]
    local dx, dy = b.x - a.x, b.y - a.y
    local len = math.sqrt(dx * dx + dy * dy)
    if len > 0.01 then
      local nx, ny = -dy / len, dx / len
      local taper = 1 - (i - 1) / n
      local a8 = math.floor(clamp(255 * alpha0 * taper, 0, 255))
      if a8 > 0 then
        local lw = math.max(0.5, bw * taper + 0.6)
        for band = 1, #BAND_HUE do
          local off = (band - (#BAND_HUE + 1) / 2) * bw * taper
          local hue = (BAND_HUE[band] + now * hue_speed) % 360
          local r, g, bl = hsv_to_rgb(hue, 1.0, 1.0)
          ctx:line(a.x + nx * off, a.y + ny * off,
                   b.x + nx * off, b.y + ny * off,
                   lw, r, g, bl, a8)
        end
      end
    end
  end
end
