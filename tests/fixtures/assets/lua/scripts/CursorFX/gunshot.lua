-- CursorFX · gunshot
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：每按一次打一枪——枪口焰、火星、烟、玻璃裂纹弹孔与抛壳；按住时按
-- fire_rate 连发。弹孔留在屏幕上，最后 1.5s 淡出。on_mouse_up 负责解除按住态，
-- step 里同时看全局 mouse_down；on_press 是包卡与官方 ClickFx 共用的入口。
-- 粒子上限：火星 200 + 烟 48 + 弹孔 16 + 弹壳 12 + 枪口焰 6。

name = "gunshot"
effect_name = "gunshot"
kind = "click"
description = "CursorFX gunshot：枪口焰、火星、烟、弹孔与抛壳，按住连发。"

local MAX_FLASHES = 6
local MAX_SPARKS = 200
local MAX_SMOKE = 48
local MAX_HOLES = 16
local MAX_CASINGS = 12

local flashes = {}
local sparks = {}
local smoke = {}
local holes = {}
local casings = {}

local held = false
local since_shot = 999.0
local aim_x, aim_y = 0.0, 0.0

function schema()
  return {
    { key = "color",     label = "枪口焰色",   type = "color", alpha = true, default = "#FFC878" },
    { key = "fire_rate", label = "连发速率",   type = "float", min = 2,   max = 20,  step = 0.5, default = 7 },
    { key = "linger",    label = "弹孔残留(秒)", type = "float", min = 1.0, max = 6.0, step = 0.1, default = 4.5 },
    { key = "sparks",    label = "每次火星数", type = "int",   min = 4,   max = 30,  step = 1,   default = 14 },
    { key = "auto",      label = "按住连发",   type = "bool",  default = true },
    { key = "cracks",    label = "玻璃裂纹",   type = "bool",  default = true },
    { key = "casings",   label = "抛壳",       type = "bool",  default = true },
  }
end

function init()
  flashes = {}
  sparks = {}
  smoke = {}
  holes = {}
  casings = {}
  held = false
  since_shot = 999.0
  aim_x, aim_y = 0.0, 0.0
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

local function trim(list, max_n)
  local over = #list - max_n
  if over > 0 then
    for i = 1, over do
      table.remove(list, 1)
    end
  end
end

local function shoot(x, y, params)
  since_shot = 0.0
  aim_x, aim_y = x, y

  local flife = 0.09
  flashes[#flashes + 1] = {
    x = x,
    y = y,
    life = flife,
    max = flife,
    rot = rng(0, 6.2831853),
  }
  trim(flashes, MAX_FLASHES)

  local n = math.floor(params.sparks or 14)
  if n < 1 then n = 1 elseif n > 30 then n = 30 end
  for i = 1, n do
    local ang = rng(0, 6.2831853)
    local sp = rng(120, 540)
    local life = rng(0.17, 0.44)
    sparks[#sparks + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp - 120,
      life = life,
      max = life,
      len = rng(3, 9),
    }
  end
  trim(sparks, MAX_SPARKS)

  for i = 1, 5 do
    local life = rng(0.8, 1.5)
    smoke[#smoke + 1] = {
      x = x + rng(-4, 4),
      y = y + rng(-4, 4),
      vx = rng(-24, 24),
      vy = rng(-72, -24),
      r = rng(3, 6),
      life = life,
      max = life,
    }
  end
  trim(smoke, MAX_SMOKE)

  local cracks = {}
  if params.cracks ~= false then
    local count = rng_int(4, 7)
    for i = 1, count do
      local a0 = (i / count) * 6.2831853 + rng(-0.3, 0.3)
      local len = rng(10, 34)
      local segs = rng_int(2, 4)
      local pts = {}
      for s = 1, segs do
        local d = (len / segs) * s
        local wob = a0 + rng(-0.25, 0.25)
        pts[#pts + 1] = math.cos(wob) * d
        pts[#pts + 1] = math.sin(wob) * d
      end
      cracks[#cracks + 1] = { pts = pts, w = rng(0.6, 1.4) }
    end
  end

  local linger = params.linger or 4.5
  if linger < 0.5 then linger = 0.5 elseif linger > 6.0 then linger = 6.0 end
  holes[#holes + 1] = {
    x = x,
    y = y,
    r = rng(4.5, 7),
    cracks = cracks,
    life = linger,
    max = linger,
    seed = rng(0, 6.2831853),
  }
  trim(holes, MAX_HOLES)

  if params.casings ~= false then
    casings[#casings + 1] = {
      x = x,
      y = y,
      vx = rng(120, 270),
      vy = rng(-300, -180),
      rot = rng(0, 6.2831853),
      spin = rng(12, 30),
      life = 1.0,
      max = 1.0,
    }
    trim(casings, MAX_CASINGS)
  end
end

function on_press(x, y, t, params)
  held = true
  aim_x, aim_y = x, y
  shoot(x, y, params)
end

function on_mouse_up(button, x, y, params)
  held = false
  aim_x, aim_y = x, y
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  since_shot = since_shot + ds

  -- 按住连发：on_mouse_up 会清 held；再与全局 mouse_down 互相校验，避免
  -- 包卡宿主与官方 ClickFx 对按住态注入时序不同时误判。
  if held and mouse_down and params.auto ~= false then
    local rate = params.fire_rate or 7.0
    if rate < 0.5 then rate = 0.5 end
    if since_shot >= 1.0 / rate then
      shoot(mouse_x or aim_x, mouse_y or aim_y, params)
    end
  end

  local n = #flashes
  local w = 0
  for i = 1, n do
    local p = flashes[i]
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      flashes[w] = p
    end
  end
  for i = #flashes, w + 1, -1 do
    flashes[i] = nil
  end

  n = #sparks
  w = 0
  local sdamp = (0.9) ^ (ds * 60.0)
  for i = 1, n do
    local p = sparks[i]
    p.vy = p.vy + 900.0 * ds
    p.vx = p.vx * sdamp
    p.vy = p.vy * sdamp
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      sparks[w] = p
    end
  end
  for i = #sparks, w + 1, -1 do
    sparks[i] = nil
  end

  n = #smoke
  w = 0
  local mdamp = (0.98) ^ (ds * 60.0)
  for i = 1, n do
    local p = smoke[i]
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.r = p.r + 21.0 * ds
    p.vy = p.vy * mdamp
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      smoke[w] = p
    end
  end
  for i = #smoke, w + 1, -1 do
    smoke[i] = nil
  end

  n = #holes
  w = 0
  for i = 1, n do
    local p = holes[i]
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      holes[w] = p
    end
  end
  for i = #holes, w + 1, -1 do
    holes[i] = nil
  end

  n = #casings
  w = 0
  for i = 1, n do
    local p = casings[i]
    p.vy = p.vy + 1260.0 * ds
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.rot = p.rot + p.spin * ds
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      casings[w] = p
    end
  end
  for i = #casings, w + 1, -1 do
    casings[i] = nil
  end
end

function draw(ctx, params)
  if #flashes == 0 and #sparks == 0 and #smoke == 0
      and #holes == 0 and #casings == 0 then
    return
  end

  local fr, fg, fb, fa = hex_to_rgb(params.color or "#FFC878")

  -- 弹孔与裂纹先画（贴在最底层），再叠烟、弹壳、火焰。
  for i = 1, #holes do
    local h = holes[i]
    local alpha = math.min(1.0, h.life / 1.5)
    if alpha > 0 then
      local ca = a8(255, alpha * 0.55)
      if #h.cracks > 0 and ca > 0 then
        for ci = 1, #h.cracks do
          local c = h.cracks[ci]
          local px, py = h.x, h.y
          for k = 1, #c.pts, 2 do
            local nx, ny = h.x + c.pts[k], h.y + c.pts[k + 1]
            ctx:line(px, py, nx, ny, c.w, 255, 255, 255, ca)
            px, py = nx, ny
          end
        end
      end

      ctx:circle(h.x, h.y, h.r * 1.9, 200, 205, 215, a8(255, alpha * 0.45))
      ctx:circle(h.x, h.y, h.r * 1.25, 70, 72, 80, a8(255, alpha * 0.9))

      local pts = {}
      for k = 0, 8 do
        local ang = (k / 9.0) * 6.2831853
        local rr = h.r * (0.8 + 0.25 * math.sin(h.seed + k * 2.1))
        pts[k + 1] = {
          h.x + math.cos(ang) * rr,
          h.y + math.sin(ang) * rr,
        }
      end
      ctx:polygon(pts, 7, 7, 10, a8(255, alpha * 0.95))
    end
  end

  for i = 1, #smoke do
    local p = smoke[i]
    local t = p.life / p.max
    local al = a8(fa, t * 0.35)
    if al > 0 then
      ctx:circle(p.x, p.y, p.r, 150, 150, 160, al)
    end
  end

  for i = 1, #casings do
    local p = casings[i]
    local alpha = math.min(1.0, p.life / 0.25)
    if alpha > 0 then
      local cr, sr = math.cos(p.rot), math.sin(p.rot)
      local local_pts = { { -4, -1.5 }, { 4, -1.5 }, { 4, 1.5 }, { -4, 1.5 } }
      local pts = {}
      for k = 1, 4 do
        local lx, ly = local_pts[k][1], local_pts[k][2]
        pts[k] = {
          p.x + lx * cr - ly * sr,
          p.y + lx * sr + ly * cr,
        }
      end
      ctx:polygon(pts, 212, 165, 58, a8(255, alpha))
      local x1, y1 = p.x + (-4) * cr - (-1.5) * sr, p.y + (-4) * sr + (-1.5) * cr
      local x2, y2 = p.x + (-2.5) * cr - (-1.5) * sr, p.y + (-2.5) * sr + (-1.5) * cr
      ctx:line(x1, y1, x2, y2, 1.5, 138, 106, 30, a8(255, alpha))
    end
  end

  for i = 1, #flashes do
    local p = flashes[i]
    local t = p.life / p.max
    if t > 0 then
      local rad = 40.0 * t + 10.0
      local stops = {
        { offset = 0.0, r = 255, g = 250, b = 220, a = a8(255, t) },
        { offset = 0.4, r = fr,  g = fg,  b = fb,  a = a8(fa, t * 0.6) },
        { offset = 1.0, r = 255, g = 120, b = 30,  a = 0 },
      }
      ctx:radial_stops(p.x, p.y, rad, stops)
      ctx:plus_circle(p.x, p.y, rad * 0.35, 255, 250, 225, a8(255, t * 0.8))

      local spike = a8(255, t)
      local len = 26.0 * t
      for k = 0, 3 do
        local ang = p.rot + k * 1.5707963
        ctx:line(
          p.x, p.y,
          p.x + math.cos(ang) * len,
          p.y + math.sin(ang) * len,
          2, 255, 240, 200, spike
        )
      end
    end
  end

  for i = 1, #sparks do
    local p = sparks[i]
    local t = p.life / p.max
    local al = a8(255, t)
    if al > 0 then
      local v = math.sqrt(p.vx * p.vx + p.vy * p.vy)
      if v < 0.001 then v = 1.0 end
      ctx:line(
        p.x, p.y,
        p.x - p.vx / v * p.len,
        p.y - p.vy / v * p.len,
        1.5,
        255, math.floor(150 + 100 * t), 60,
        al
      )
      ctx:plus_circle(p.x, p.y, 1.4 * t + 0.3, 255, 200, 90, a8(255, t * 0.7))
    end
  end
end
