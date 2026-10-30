-- MFCMouseEffect · Neon HUD (3D)
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 霓虹 HUD：玻璃环 + 进度弧 + 编织带 + 胶囊头 + 百分比 + 少量触须。
-- 上游是极复杂的多层 HUD；本包保留可读的主体层。已登记的简化实际去除范围：
-- 最密编织纹理、内扫描线（DrawInnerScanner）、水晶种（DrawCrystalSeed）、
-- 分支触须火花（6–9 分支），以及 120ms 出现时的 scale-in（0.90→1.0，EaseOutBack）。

name = "Neon HUD (3D)"
effect_name = "Neon HUD (3D)"
kind = "hold"
description = "MFCMouseEffect Neon HUD：按住时展开的玻璃环 HUD 与百分比。"

local HOLD_DELAY = 260
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0

function schema()
  return {
    { key = "color",    label = "主色",     type = "color", alpha = true, default = "#4DF8FF" },
    { key = "accent",   label = "辅色",     type = "color", alpha = true, default = "#FF5AF7" },
    { key = "mint",     label = "点缀色",   type = "color", alpha = true, default = "#7CFFC4" },
    { key = "size",     label = "HUD 半径", type = "float", min = 30, max = 160, step = 2,   default = 72 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
    { key = "label",    label = "百分比文字", type = "bool", default = true },
  }
end

local function smoothstep(a, b, x)
  if b <= a then return x >= b and 1 or 0 end
  local t = clamp((x - a) / (b - a), 0, 1)
  return t * t * (3 - 2 * t)
end

function init()
  press_at = nil
  vis = 0
  progress = 0
  hx, hy = mouse_x or 0, mouse_y or 0
end

function on_mouse_down(button, x, y, params)
  press_at = wall_ms or 0
  hx, hy = x, y
end

function on_mouse_up(button, x, y, params)
  press_at = nil
end

function step(dt, t, params)
  local now = wall_ms or 0
  local k = math.min(1.0, (params.follow or 0.25) * dt * 60.0)
  hx = hx + ((mouse_x or hx) - hx) * k
  hy = hy + ((mouse_y or hy) - hy) * k
  if press_at and mouse_down then
    local held = now - press_at
    if held >= HOLD_DELAY then
      progress = clamp(held / math.max(300, params.duration or 900), 0, 1)
      vis = vis + (1 - vis) * math.min(1.0, dt * 10.0)
    end
  else
    vis = vis - vis * math.min(1.0, dt * 8.0)
    if vis < 0.01 then vis = 0 end
  end
end

local function arc(ctx, cx, cy, radius, start_deg, sweep_deg, width, r, g, b, a)
  if sweep_deg <= 0.01 then return end
  local steps = math.max(6, math.floor(sweep_deg / 4.0))
  local prev_x, prev_y
  for i = 0, steps do
    local ang = math.rad(start_deg + sweep_deg * i / steps)
    local x = cx + math.cos(ang) * radius
    local y = cy + math.sin(ang) * radius
    if i > 0 then ctx:line(prev_x, prev_y, x, y, width, r, g, b, a) end
    prev_x, prev_y = x, y
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local pr, pg, pb, pa = hex_to_rgb(params.accent or "#FF5AF7")
  local mr, mg, mb, ma = hex_to_rgb(params.mint or "#7CFFC4")
  local duration = math.max(300, params.duration or 900)
  local held = press_at and ((wall_ms or 0) - press_at) or duration * progress
  local appear = clamp(held / 120.0, 0, 1)
  local alpha_in = smoothstep(0, 1, appear) * vis
  local scale_in = 0.90 + 0.10 * appear
  local radius = (params.size or 72) * scale_in
  local thick = math.max(12.0, radius * 0.18)
  local inner = radius - thick * 0.45
  local head_deg = -90.0 + progress * 360.0

  -- Glass ring: shell + inner shadow + fog + rim highlights + purple tint.
  ctx:ring(hx, hy, radius, thick, r, g, b, math.floor(ca * 0.16 * alpha_in))
  ctx:ring(hx, hy, radius - thick * 0.30, thick * 0.65, 0, 0, 0,
    math.floor(255 * 0.18 * alpha_in))
  ctx:ring(hx, hy, radius, thick + 14, r, g, b, math.floor(ca * 0.06 * alpha_in))
  ctx:ring(hx, hy, radius, thick + 8, r, g, b, math.floor(ca * 0.10 * alpha_in))
  ctx:ring(hx, hy, radius, thick + 20, r, g, b, math.floor(ca * 0.045 * alpha_in))
  ctx:ring(hx, hy, radius - thick * 0.45, 2.0, 255, 255, 255,
    math.floor(255 * 0.22 * alpha_in))
  ctx:ring(hx, hy, radius + thick * 0.45, 2.0, r, g, b,
    math.floor(ca * 0.18 * alpha_in))
  ctx:ring(hx, hy, radius, thick * 0.55, pr, pg, pb, math.floor(pa * 0.06 * alpha_in))

  -- Micro streaks on the inner ring (12 spokes).
  local streak_a = math.floor(ca * 0.12 * alpha_in)
  for i = 0, 11 do
    local ang = math.rad(i * 30 + (wall_ms or 0) * 0.01)
    local x1 = hx + math.cos(ang) * (inner + 3)
    local y1 = hy + math.sin(ang) * (inner + 3)
    local x2 = hx + math.cos(ang) * (inner - 7)
    local y2 = hy + math.sin(ang) * (inner - 7)
    ctx:line(x1, y1, x2, y2, 1.0, r, g, b, streak_a)
  end

  -- Progress arcs.
  local p_a = math.floor(ca * 0.10 * alpha_in)
  arc(ctx, hx, hy, radius, -90, progress * 360, 2.0, r, g, b, p_a)
  local core_fade = 1 - smoothstep(0.25, 0.55, progress)
  arc(ctx, hx, hy, radius, -90, progress * 360, 14.0, pr, pg, pb,
    math.floor(pa * 0.025 * alpha_in))
  arc(ctx, hx, hy, radius, -90, progress * 360, 10.0, r, g, b,
    math.floor(ca * 0.040 * alpha_in))
  arc(ctx, hx, hy, radius, -90, progress * 360, 6.5, r, g, b,
    math.floor(ca * 0.075 * alpha_in))
  arc(ctx, hx, hy, radius, -90, progress * 360, 2.6, 235, 248, 255,
    math.floor(255 * 0.28 * core_fade * alpha_in))

  -- Start anchor plate at 12 o'clock.
  arc(ctx, hx, hy, radius, -90 - 5.2, 10.4, 7.5, pr, pg, pb,
    math.floor(pa * 0.05 * alpha_in * 0.85))
  arc(ctx, hx, hy, radius, -90 - 5.2, 10.4, 3.8, r, g, b,
    math.floor(ca * 0.13 * alpha_in * 0.85))
  arc(ctx, hx, hy, radius, -90 - 5.2, 10.4, 1.2, 245, 252, 255,
    math.floor(255 * 0.18 * alpha_in * 0.85))

  -- Woven band near the head (5 short arcs).
  local weave_a = math.floor(ca * 0.16 * smoothstep(0.10, 0.30, progress) * alpha_in)
  for i = 0, 4 do
    local start = head_deg - 28.0 + i * 5.0
    local cr, cg, cb = r, g, b
    local aa = weave_a
    if i % 3 == 1 then cr, cg, cb = pr, pg, pb end
    if i % 3 == 2 then cr, cg, cb = mr, mg, mb; aa = math.floor(weave_a * ma / 255) end
    arc(ctx, hx, hy, radius, start, 3.5, 3.0, cr, cg, cb, aa)
  end

  -- Capsule head.
  local capsule_a = smoothstep(0.12, 0.28, progress) * alpha_in
  if progress > 0.001 then
    arc(ctx, hx, hy, radius, head_deg - 26.0, 26.0, 18.0, pr, pg, pb,
      math.floor(pa * 0.06 * capsule_a))
    arc(ctx, hx, hy, radius, head_deg - 26.0, 26.0, 12.0, pr, pg, pb,
      math.floor(pa * 0.16 * capsule_a))
    arc(ctx, hx, hy, radius, head_deg - 26.0, 26.0, 12.0, r, g, b,
      math.floor(ca * 0.07 * capsule_a))
    arc(ctx, hx, hy, radius, head_deg - 26.0, 26.0, 8.0, r, g, b,
      math.floor(ca * 0.20 * capsule_a))
    arc(ctx, hx, hy, radius, head_deg - 26.0, 26.0, 3.2, 245, 250, 255,
      math.floor(255 * 0.90 * capsule_a))
    local ha = math.rad(head_deg)
    local hxp = hx + math.cos(ha) * radius
    local hyp = hy + math.sin(ha) * radius
    ctx:circle(hxp, hyp, 2.6, 245, 250, 255, math.floor(255 * 0.95 * capsule_a))
    ctx:circle(hxp, hyp, 7.0, r, g, b, math.floor(ca * 0.18 * capsule_a))
  end

  -- Tendrils from the head (5 short curved lines).
  local tendril_a = math.floor(ca * 0.5 * smoothstep(0.08, 0.26, progress) * alpha_in)
  for i = 1, 5 do
    local base_ang = math.rad(head_deg - 55.0 + i * 9.0)
    local prev_x = hx + math.cos(base_ang) * inner
    local prev_y = hy + math.sin(base_ang) * inner
    for j = 1, 3 do
      local ang = base_ang + j * 0.06 * (i % 2 == 0 and 1 or -1)
      local rad = inner - j * 9.0
      local x = hx + math.cos(ang) * rad
      local y = hy + math.sin(ang) * rad
      ctx:line(prev_x, prev_y, x, y, math.max(0.8, 2.6 - j * 0.5),
        pr, pg, pb, math.floor(tendril_a * (0.8 - j * 0.15)))
      prev_x, prev_y = x, y
    end
  end

  if params.label ~= false and progress > 0.001 then
    local label_a = smoothstep(0.08, 0.22, progress) * alpha_in
    local la = math.rad(head_deg)
    local lr = radius + 20.0
    ctx:text_centered(hx + math.cos(la) * lr, hy + math.sin(la) * lr,
      string.format("%d%%", math.floor(progress * 100 + 0.5)), 12,
      r, g, b, math.floor(255 * label_a))
  end
end
