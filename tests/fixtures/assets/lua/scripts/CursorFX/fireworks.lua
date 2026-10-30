-- CursorFX · fireworks
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：一枚火箭从按下点下方升空，拖一条暖色尾迹，到点后炸成一圈彩色火星。
-- 火星用“当前点 → 速度反方向”的渐变短线模拟原版的彗尾，颜色由 HSV 现算。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。

name = "fireworks"
effect_name = "fireworks"
kind = "click"
description = "CursorFX fireworks：升空火箭与彩色火星爆炸。"

local MAX_ROCKETS = 8
local MAX_SPARKS = 280

local rockets = {}
local sparks = {}

function schema()
  return {
    { key = "rainbow", label = "随机色相", type = "bool",  default = true },
    { key = "hue",     label = "固定色相", type = "int",   min = 0,   max = 360, step = 5,   default = 30 },
    { key = "sparks",  label = "火星数",   type = "int",   min = 20,  max = 160, step = 5,   default = 80 },
    { key = "size",    label = "火星大小", type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
  }
end

function init()
  rockets = {}
  sparks = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

local function norm_hue(h)
  h = h % 360.0
  if h < 0 then h = h + 360.0 end
  return h
end

local function trim_sparks()
  local over = #sparks - MAX_SPARKS
  if over > 0 then
    for i = 1, over do
      table.remove(sparks, 1)
    end
  end
end

local function add_spark(x, y, vx, vy, life, hue, trail)
  sparks[#sparks + 1] = {
    x = x,
    y = y,
    vx = vx,
    vy = vy,
    life = life,
    max = life,
    hue = hue,
    trail = trail,
  }
end

local function burst(x, y, hue, params)
  local n = math.floor(params.sparks or 80)
  if n < 20 then n = 20 elseif n > 160 then n = 160 end
  for i = 1, n do
    local ang = rng(0, 6.2831853)
    local sp = rng(60, 420) * (params.size or 1.0)
    local life = rng(0.67, 1.33)
    add_spark(
      x, y,
      math.cos(ang) * sp,
      math.sin(ang) * sp,
      life,
      hue + rng(-20, 20),
      1.0
    )
  end
  trim_sparks()
end

function on_press(x, y, t, params)
  local rainbow = params.rainbow ~= false
  local hue = rainbow and rng(0, 360) or (params.hue or 30)
  local sx = x + rng(-40, 40)
  local sy = math.min((screen_h or 720) + 10, y + rng(180, 320))
  rockets[#rockets + 1] = {
    x = sx,
    y = sy,
    sx = sx,
    sy = sy,
    tx = x,
    ty = y,
    t = 0,
    hue = hue,
    emit = 0,
  }
  while #rockets > MAX_ROCKETS do
    table.remove(rockets, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local rn = #rockets
  local rw = 0
  for i = 1, rn do
    local r = rockets[i]
    r.t = r.t + 2.1 * ds
    local u = r.t
    if u > 1.0 then u = 1.0 end
    local e = 1.0 - (1.0 - u) * (1.0 - u)
    r.x = r.sx + (r.tx - r.sx) * e
    r.y = r.sy + (r.ty - r.sy) * e

    r.emit = r.emit + ds
    while r.emit >= 0.0166667 do
      r.emit = r.emit - 0.0166667
      add_spark(
        r.x + rng(-2, 2),
        r.y + rng(-2, 2),
        rng(-24, 24),
        rng(30, 90),
        rng(0.17, 0.33),
        40,
        0.6
      )
    end

    if r.t >= 1.0 then
      burst(r.tx, r.ty, r.hue, params)
    else
      rw = rw + 1
      rockets[rw] = r
    end
  end
  for i = #rockets, rw + 1, -1 do
    rockets[i] = nil
  end

  local n = #sparks
  local w = 0
  for i = 1, n do
    local p = sparks[i]
    p.vy = p.vy + 216.0 * ds
    local damp = (0.96) ^ (ds * 60.0)
    p.vx = p.vx * damp
    p.vy = p.vy * damp
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

  trim_sparks()
end

function draw(ctx, params)
  if #rockets == 0 and #sparks == 0 then return end

  for i = 1, #rockets do
    local r = rockets[i]
    ctx:line(r.x, r.y + 11, r.x, r.y + 2, 2, 255, 220, 150, 120)
    ctx:plus_circle(r.x, r.y, 8, 255, 210, 140, 42)
    ctx:circle(r.x, r.y, 2.4, 255, 255, 255, 255)
  end

  for i = 1, #sparks do
    local p = sparks[i]
    local t = p.life / p.max
    local al = a8(255, t)
    if al > 0 then
      local hue = norm_hue(p.hue)
      local r, g, b = hsv_to_rgb(hue, 1.0, 0.6 + 0.25 * t)
      local tail = 0.05 * p.trail
      ctx:line(
        p.x - p.vx * tail,
        p.y - p.vy * tail,
        p.x, p.y,
        1.5 * t + 0.5,
        r, g, b, a8(255, t * 0.85)
      )
      ctx:plus_circle(p.x, p.y, 1.6 * t + 0.35, r, g, b, al)
    end
  end
end
