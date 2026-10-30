-- CursorFX · butterflies
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：一群蝴蝶从按下点炸开，扇着翅膀、带着随机游走飞远。两对翅膀用原版的
-- 两段三次贝塞尔曲线采样成多边形，再按航向旋转；身体/触角用短线，白斑用小圆。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。

name = "butterflies"
effect_name = "butterflies"
kind = "click"
description = "CursorFX butterflies：扇翅飞舞的彩色蝴蝶群。"

local MAX_BUTTERFLIES = 160
local PALETTE = { "#FF8FAB", "#FFD166", "#8ECAE6", "#C77DFF", "#80ED99", "#FFB703" }

local butterflies = {}

function schema()
  return {
    { key = "color",   label = "单色",     type = "color", alpha = true, default = "#FF8FAB" },
    { key = "rainbow", label = "原版配色", type = "bool",  default = true },
    { key = "count",   label = "蝴蝶数",   type = "int",   min = 1,  max = 30, step = 1,   default = 10 },
    { key = "size",    label = "蝴蝶大小", type = "float", min = 6,  max = 30, step = 0.5, default = 14 },
  }
end

function init()
  butterflies = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

local function cubic(x0, y0, x1, y1, x2, y2, x3, y3, u)
  local v = 1.0 - u
  local a = v * v * v
  local b = 3.0 * v * v * u
  local c = 3.0 * v * u * u
  local d = u * u * u
  return a * x0 + b * x1 + c * x2 + d * x3,
         a * y0 + b * y1 + c * y2 + d * y3
end

local function place(px, py, ca, sa, lx, ly)
  return px + lx * ca - ly * sa, py + lx * sa + ly * ca
end

local function add_wing(out, px, py, s, flap, side, upper, ca, sa)
  local function add(lx, ly)
    local wx, wy = place(px, py, ca, sa, lx * s, ly * s)
    out[#out + 1] = { wx, wy }
  end

  add(0, 0)
  local m = side
  if upper then
    for i = 1, 4 do
      local x, y = cubic(
        0, 0,
        m * 0.6 * flap, -0.9,
        m * 1.4 * flap, -0.4,
        m * 1.2 * flap, 0.25,
        i / 4.0
      )
      add(x, y)
    end
    for i = 1, 4 do
      local x, y = cubic(
        m * 1.2 * flap, 0.25,
        m * 0.9 * flap, 0.55,
        m * 0.3 * flap, 0.4,
        0, 0,
        i / 4.0
      )
      add(x, y)
    end
  else
    for i = 1, 4 do
      local x, y = cubic(
        0, 0,
        m * 0.6 * flap, 0.45,
        m * 1.4 * flap, 0.24,
        m * 1.2 * flap, 0.4,
        i / 4.0
      )
      add(x, y)
    end
    for i = 1, 4 do
      local x, y = cubic(
        m * 1.2 * flap, 0.4,
        m * 0.9 * flap, 0.825,
        m * 0.3 * flap, 0.4,
        0, 0,
        i / 4.0
      )
      add(x, y)
    end
  end
end

function on_press(x, y, t, params)
  local n = math.floor(params.count or 10)
  if n < 1 then n = 1 elseif n > 30 then n = 30 end
  local base = params.size or 14
  local rainbow = params.rainbow ~= false
  local fixed = params.color or "#FF8FAB"

  for i = 1, n do
    local ang = rng(0, 6.2831853)
    local sp = rng(90, 240)
    local life = rng(2.5, 4.33)
    butterflies[#butterflies + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp - 60,
      a = ang + 1.5707963,
      size = base * rng(0.7, 1.3),
      color = rainbow and PALETTE[rng_int(1, #PALETTE)] or fixed,
      flap = rng(0, 6.2831853),
      fs = rng(21, 36),
      wander = rng(0, 6.2831853),
      life = life,
      max = life,
    }
  end

  while #butterflies > MAX_BUTTERFLIES do
    table.remove(butterflies, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local n = #butterflies
  local w = 0
  for i = 1, n do
    local p = butterflies[i]
    p.wander = p.wander + 3.0 * ds
    p.vx = p.vx + math.cos(p.wander * 1.7) * 432.0 * ds
    p.vy = p.vy + (math.sin(p.wander * 1.3) * 432.0 - 72.0) * ds
    local damp = (0.985) ^ (ds * 60.0)
    p.vx = p.vx * damp
    p.vy = p.vy * damp
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.flap = p.flap + p.fs * ds
    if p.vx * p.vx + p.vy * p.vy > 25.0 then
      p.a = math.atan(p.vy, p.vx) + 1.5707963
    end
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      butterflies[w] = p
    end
  end
  for i = #butterflies, w + 1, -1 do
    butterflies[i] = nil
  end
end

function draw(ctx, params)
  if #butterflies == 0 then return end

  for i = 1, #butterflies do
    local p = butterflies[i]
    local t = p.life / p.max
    local al = a8(255, math.min(1.0, t * 3.0))
    if al > 0 then
      local r, g, b, ca = hex_to_rgb(p.color)
      al = a8(ca, al / 255.0)
      local cr, sr = math.cos(p.a), math.sin(p.a)
      local flap = 0.35 + 0.65 * math.abs(math.cos(p.flap))
      local s = p.size

      for side = -1, 1, 2 do
        local upper = {}
        add_wing(upper, p.x, p.y, s, flap, side, true, cr, sr)
        ctx:polygon(upper, r, g, b, al)

        local lower = {}
        add_wing(lower, p.x, p.y, s, flap, side, false, cr, sr)
        ctx:polygon(lower, r, g, b, a8(al, 0.9))

        local wx, wy = place(p.x, p.y, cr, sr, side * s * 0.7 * flap, -s * 0.3)
        ctx:circle(wx, wy, s * 0.14, 255, 255, 255, a8(al, 0.45))
      end

      -- 身体：用一条粗短线代替旋转椭圆（CurSeen 的 ellipse 不能旋转）。
      local bx1, by1 = place(p.x, p.y, cr, sr, 0, -s * 0.5)
      local bx2, by2 = place(p.x, p.y, cr, sr, 0, s * 0.55)
      ctx:line(bx1, by1, bx2, by2, math.max(1.0, s * 0.22), 43, 43, 58, al)

      -- 两根触角。
      for side = -1, 1, 2 do
        local ax, ay = place(p.x, p.y, cr, sr, side * s * 0.3, -s * 0.95)
        ctx:line(bx1, by1, ax, ay, 1, 43, 43, 58, al)
      end
    end
  end
end
