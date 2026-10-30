-- MouseEffects · Firework
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击烟花：上游默认在点击点原位爆（_enableRocketMode 默认 false）；升空火箭
-- 是可选模式（本包 rocket 参数，默认关）。上游 Styles/ 有 19 种爆炸样式，这里
-- 实现球/环/垂柳/棕榈 4 种常见形随机取一；火星池 260。

name = "Firework"
effect_name = "Firework"
kind = "click"
description = "MouseEffects Firework：原位/升空两种模式与四样式彩色爆炸。"

local MAX_ROCKETS = 6
local MAX_SPARKS = 260
local rockets = {}
local sparks = {}

function schema()
  return {
    { key = "sparks",  label = "火星数",   type = "int",   min = 30,  max = 160, step = 5,   default = 80 },
    { key = "force",   label = "爆炸力",   type = "float", min = 100, max = 500, step = 10,  default = 260 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 0.8, max = 2.5, step = 0.1, default = 1.6 },
    { key = "gravity", label = "重力",     type = "float", min = 60,  max = 240, step = 10,  default = 120 },
    { key = "rise",    label = "升空时间(秒)", type = "float", min = 0.3, max = 1.5, step = 0.1, default = 0.8 },
    { key = "rocket",  label = "升空火箭", type = "bool",  default = false },
    { key = "rainbow", label = "随机颜色", type = "bool",  default = true },
    { key = "color",   label = "固定颜色", type = "color", default = "#FFC878" },
  }
end

function init()
  rockets = {}
  sparks = {}
end

local function add_spark(x, y, vx, vy, life, r, g, b, am)
  if #sparks >= MAX_SPARKS then table.remove(sparks, 1) end
  sparks[#sparks + 1] = {
    x = x, y = y, vx = vx, vy = vy,
    age = 0, life = life, r = r, g = g, b = b,
    tw = rng(0, math.pi * 2),
    alpha_mul = am,
  }
end

local function explode(x, y, params, scale, hue)
  local n = math.floor((params.sparks or 80) * scale)
  local force = (params.force or 260) * scale
  local life = (params.life or 1.6) * rng(0.8, 1.1)
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFC878")
  local am = ca / 255
  -- 上游 Styles/ 有 19 种爆炸样式；实现 4 种常见形（球/环/垂柳/棕榈）随机取一，
  -- 其余样式在 README 登记为压缩。
  local style = rng_int(1, 4)
  if style == 4 then n = math.floor(n * 0.45) end
  for i = 1, n do
    local a = rng(0, math.pi * 2)
    local sp
    if style == 2 then
      sp = force * rng(0.9, 1.0)
    elseif style == 3 then
      sp = force * rng(0.3, 0.6)
    else
      sp = force * rng(0.25, 1.0)
    end
    local r, g, b
    if params.rainbow == false then
      r, g, b = cr, cg, cb
    else
      local h = (hue + rng(-25, 25)) % 360
      r, g, b = hsv_to_rgb(h, 1.0, 1.0)
    end
    local l = life * rng(0.7, 1.15)
    if style == 3 then l = l * 1.6 end
    add_spark(x, y, math.cos(a) * sp, math.sin(a) * sp, l, r, g, b, am)
  end
end

function on_press(x, y, t, params)
  params = params or {}
  if params.rocket then
    -- 升空模式（可选，上游 _enableRocketMode 打开后才有）。
    if #rockets >= MAX_ROCKETS then table.remove(rockets, 1) end
    rockets[#rockets + 1] = {
      x = x, y = y + 180, y0 = y + 180,
      tx = x, ty = y,
      age = 0,
      dur = params.rise or 0.8,
      hue = rng(0, 360),
      trail = {},
    }
  else
    -- 上游默认原位爆（FireworkEffect.cs SpawnFirework 原位分支）。
    local hue = rng(0, 360)
    explode(x, y, params, 1.0, hue)
    if rng(0, 1) < 0.35 then
      explode(x + rng(-30, 30), y + rng(-30, 30), params, 0.35, hue + 40)
    end
  end
end

function step(dt, t, params)
  params = params or {}
  local gravity = params.gravity or 120
  for i = #rockets, 1, -1 do
    local r = rockets[i]
    r.age = r.age + dt
    local u = math.min(1.0, r.age / r.dur)
    r.trail[#r.trail + 1] = { r.x, r.y }
    if #r.trail > 8 then table.remove(r.trail, 1) end
    r.y = r.y0 + (r.ty - r.y0) * u
    if u >= 1.0 then
      explode(r.tx, r.ty, params, 1.0, r.hue)
      -- 少量二次爆炸：在第一次的火星云里再开一朵小花。
      if rng(0, 1) < 0.35 then
        explode(r.tx + rng(-30, 30), r.ty + rng(-30, 30), params, 0.35, r.hue + 40)
      end
      table.remove(rockets, i)
    end
  end

  for i = #sparks, 1, -1 do
    local s = sparks[i]
    s.age = s.age + dt
    if s.age >= s.life then
      table.remove(sparks, i)
    else
      s.vy = s.vy + gravity * dt
      s.vx = s.vx * math.pow(0.96, dt * 60)
      s.vy = s.vy * math.pow(0.99, dt * 60)
      s.x = s.x + s.vx * dt
      s.y = s.y + s.vy * dt
    end
  end
end

function draw(ctx, params)
  params = params or {}
  for i = 1, #rockets do
    local r = rockets[i]
    for k = 2, #r.trail do
      local p0, p1 = r.trail[k - 1], r.trail[k]
      ctx:line(p0[1], p0[2], p1[1], p1[2], 2.0, 255, 200, 120, 160)
    end
    ctx:plus_circle(r.x, r.y, 3.2, 255, 230, 170, 240)
  end

  for i = 1, #sparks do
    local s = sparks[i]
    local k = 1 - s.age / s.life
    local tw = 0.75 + 0.25 * math.sin((elapsed or 0) * 18.0 + s.tw)
    ctx:plus_circle(s.x, s.y, 2.0 * tw, s.r, s.g, s.b, math.floor(235 * k * s.alpha_mul))
  end
end
