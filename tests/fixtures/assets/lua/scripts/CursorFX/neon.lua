-- CursorFX · neon
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：霓虹灯管沿指针路径延伸，色相随时间流动，尾端按寿命淡出。没有 shadowBlur，
-- 用三遍由粗到细的线段（低透明度宽光晕 + 中等光晕 + 实心灯芯）模拟辉光。

name = "neon"
effect_name = "neon"
kind = "trail"
description = "CursorFX neon：流光色相的霓虹灯管拖尾。"

local MAX_POINTS = 240
local PASSES = {
  { width = 5.0, alpha = 0.12 },
  { width = 2.2, alpha = 0.35 },
  { width = 1.0, alpha = 1.00 },
}

local points = {}

function schema()
  return {
    { key = "width",     label = "灯管粗细", type = "float", min = 1,   max = 12,  step = 0.5, default = 4 },
    { key = "life",      label = "存留时间", type = "float", min = 0.2, max = 2.0, step = 0.05, default = 0.7 },
    { key = "hue_speed", label = "色相流速", type = "float", min = 0,   max = 180, step = 5,   default = 60 },
    { key = "rainbow",   label = "流光色相", type = "bool",  default = true },
    { key = "color",     label = "固定色",   type = "color", alpha = true, default = "#00E5FF" },
  }
end

function init()
  points = {}
end

function on_move(x, y, t, params)
  local last = points[#points]
  if last then
    local dx, dy = x - last.x, y - last.y
    if dx * dx + dy * dy <= 4 then return end
  end

  points[#points + 1] = { x = x, y = y, t = t or elapsed or 0 }
  if #points > MAX_POINTS then
    table.remove(points, 1)
  end
end

function step(dt, t, params)
  local life = params.life or 0.7
  if #points == 0 then return end

  local now = t or elapsed or 0
  local keep = {}
  for i = 1, #points do
    local p = points[i]
    if now - p.t <= life then
      keep[#keep + 1] = p
    end
  end
  points = keep
end

function draw(ctx, params)
  if #points < 2 then return end

  local now = elapsed or 0
  local width = params.width or 4
  local life = params.life or 0.7
  local hue_speed = params.hue_speed or 60
  local rainbow = params.rainbow ~= false
  local fr, fg, fb, fa = hex_to_rgb(params.color or "#00E5FF")
  local base_a = fa / 255
  local n = #points

  for pass = 1, #PASSES do
    local def = PASSES[pass]
    for i = 2, n do
      local age = (now - points[i].t) / life
      if age < 1 then
        local alpha = (1 - age) * def.alpha * base_a
        if alpha > 0.01 then
          local r, g, b
          if rainbow then
            local hue = (now * hue_speed + i * 1.5) % 360
            r, g, b = hsv_to_rgb(hue, 1.0, pass == 3 and 0.85 or 0.6)
          else
            r, g, b = fr, fg, fb
          end
          local a8 = math.floor(clamp(alpha, 0, 1) * 255)
          local lw = width * def.width * (1 - age * 0.6)
          if a8 > 0 and lw >= 0.2 then
            ctx:line(points[i - 1].x, points[i - 1].y,
                     points[i].x, points[i].y,
                     lw, r, g, b, a8)
          end
        end
      end
    end
  end

  -- 灯头加一团小辉光，强调“灯管还活着”。
  local head = points[n]
  local hr, hg, hb
  if rainbow then
    hr, hg, hb = hsv_to_rgb((now * hue_speed + n * 1.5) % 360, 1.0, 0.85)
  else
    hr, hg, hb = fr, fg, fb
  end
  local glow_a = math.floor(70 * base_a)
  if glow_a > 0 then
    ctx:plus_circle(head.x, head.y, width * 2.0, hr, hg, hb, glow_a)
  end
end
