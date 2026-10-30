-- MouseEffects · Ink Blot
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击处晕开的墨迹：一团主墨 + 若干卫星墨点，边缘用径向渐变做晕染，
-- 随寿命缓慢扩散、变淡；墨点受重力下落，近距墨点按表面张力互吸、接触后
-- 融合（上游 metaball "drip and merge organically" 的软圆近似）。

name = "Ink Blot"
effect_name = "Ink Blot"
kind = "click"
description = "MouseEffects Ink Blot：点击晕开的墨迹。"

local MAX_BLOBS = 60
local blobs = {}

function schema()
  return {
    { key = "blobs",   label = "墨点数",   type = "int",   min = 6,   max = 28,  step = 1,   default = 14 },
    { key = "size",    label = "墨团大小", type = "float", min = 10,  max = 64,  step = 2,   default = 28 },
    { key = "spread",  label = "扩散范围", type = "float", min = 6,   max = 60,  step = 2,   default = 24 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 1,   max = 6,   step = 0.1, default = 3.0 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.3, max = 1,   step = 0.05, default = 0.85 },
    { key = "glow",    label = "边缘柔光", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.2 },
    { key = "color",   label = "墨色",     type = "color", default = "#1B1B2F" },
  }
end

function init()
  blobs = {}
end

function on_press(x, y, t, params)
  params = params or {}
  blobs = {}
  local n = math.floor(params.blobs or 14)
  local spread = params.spread or 24
  for i = 1, n do
    local a = rng(0, math.pi * 2)
    local d = i == 1 and 0 or rng(spread * 0.3, spread * 1.8)
    blobs[#blobs + 1] = {
      x = x + math.cos(a) * d,
      y = y + math.sin(a) * d,
      r = (params.size or 28) * (i == 1 and rng(0.8, 1.1) or rng(0.25, 0.7)),
      grow = rng(0.6, 1.4),
      age = 0,
      life = (params.life or 3.0) * rng(0.75, 1.15),
      phase = rng(0, math.pi * 2),
      vy = 0,
    }
  end
  while #blobs > MAX_BLOBS do table.remove(blobs, 1) end
end

function step(dt, t, params)
  params = params or {}
  for i = #blobs, 1, -1 do
    local b = blobs[i]
    b.age = b.age + dt
    if b.age >= b.life then
      table.remove(blobs, i)
    else
      b.r = b.r + b.grow * dt
      b.x = b.x + math.sin(b.phase + b.age * 0.7) * 2.0 * dt
      b.y = b.y + math.cos(b.phase + b.age * 0.9) * 1.5 * dt
      -- 重力下落（上游 InkBlotEffect.cs:335）。
      b.vy = b.vy + 220 * dt
      b.y = b.y + b.vy * dt
    end
  end

  -- 表面张力：近距墨点互吸，接触后按面积守恒融合进大者（上游 :312 表面张力）。
  for i = 1, #blobs do
    local bi = blobs[i]
    for j = #blobs, i + 1, -1 do
      local bj = blobs[j]
      local dx, dy = bj.x - bi.x, bj.y - bi.y
      local d2 = dx * dx + dy * dy
      local touch = (bi.r + bj.r) * 0.9
      if d2 < touch * touch and d2 > 0.01 then
        local d = math.sqrt(d2)
        local pull = 26 * dt / d
        bi.x = bi.x + dx * pull
        bi.y = bi.y + dy * pull
        bj.x = bj.x - dx * pull
        bj.y = bj.y - dy * pull
        if d < math.max(bi.r, bj.r) * 0.55 then
          local big, small = bi, bj
          if bj.r > bi.r then big, small = bj, bi end
          big.r = math.sqrt(big.r * big.r + small.r * small.r)
          table.remove(blobs, j)
        end
      end
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#1B1B2F")
  local am = ca / 255
  local opacity = params.opacity or 0.85
  local glow = params.glow or 0.2
  for i = 1, #blobs do
    local b = blobs[i]
    local k = 1 - b.age / b.life
    local a = math.floor(210 * opacity * k * am)
    local inner = math.floor(245 * opacity * k * am)
    ctx:radial_stops(b.x, b.y, b.r, {
      { offset = 0.0, r = cr, g = cg, b = cb, a = inner },
      { offset = 0.62, r = cr, g = cg, b = cb, a = math.floor(a * 0.85) },
      { offset = 1.0, r = cr, g = cg, b = cb, a = 0 },
    })
    if glow > 0.01 and i % 4 == 0 then
      ctx:glow(b.x, b.y, b.r * 0.6, cr, cg, cb, math.floor(40 * glow * k * am))
    end
  end
end
