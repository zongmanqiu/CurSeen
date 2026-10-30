-- CursorFX · ink
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：按下点先炸开一圈墨滴，中心墨团为正圆（原版 ink.js 所有粒子均用
-- arc 正圆绘制，不做不规则墨渍形），部分墨滴带向下的滴落尾巴；全部元素按
-- 寿命淡出。原版按帧计时，这里换成秒，扩散节奏保持一致。

name = "ink"
effect_name = "ink"
kind = "click"
description = "CursorFX ink：按下点的墨团扩散与墨滴滴落。"

local MAX_PARTICLES = 300
local particles = {}

local function add_splat(x, y, size)
  -- 原版中心墨团是正圆（ink.js:33 arc），不再用 13 顶点不规则多边形。
  local target = size * rng(0.8, 1.2)
  particles[#particles + 1] = {
    kind = "splat",
    x = x,
    y = y,
    r = target * 0.1,
    target = target,
    life = 1.9,
    max = 1.9,
  }
end

local function add_blob(x, y, size)
  local a = rng(0, math.pi * 2)
  local d = rng(0.3, 1.4) * size
  local life = rng(1.35, 1.9)
  particles[#particles + 1] = {
    kind = "blob",
    x = x,
    y = y,
    tx = x + math.cos(a) * d,
    ty = y + math.sin(a) * d,
    r = 0,
    target = rng(2, 9),
    life = life,
    max = life,
    drip = rng(0, 1) < 0.3,
    dy = 0,
  }
end

function schema()
  return {
    { key = "color", label = "墨色",   type = "color", alpha = true, default = "#1B1B2F" },
    { key = "blobs", label = "墨滴数", type = "int",   min = 4,  max = 28, step = 1, default = 14 },
    { key = "size",  label = "墨团大小", type = "float", min = 10, max = 64, step = 1, default = 26 },
  }
end

function init()
  particles = {}
end

function on_press(x, y, t, params)
  local size = params.size or 26
  local count = math.floor(params.blobs or 14)
  add_splat(x, y, size)
  for _ = 1, count do
    if #particles >= MAX_PARTICLES then
      break
    end
    add_blob(x, y, size)
  end
end

function step(dt, t, params)
  if #particles == 0 then
    return
  end
  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      if p.kind == "splat" then
        -- 原版墨团扩散 0.25/帧（60fps 折算 dt*15），旧值 dt*8 偏慢。
        p.r = p.r + (p.target - p.r) * math.min(1, dt * 15)
      else
        p.r = p.r + (p.target - p.r) * math.min(1, dt * 10)
        p.x = p.x + (p.tx - p.x) * math.min(1, dt * 9)
        p.y = p.y + (p.ty - p.y) * math.min(1, dt * 9)
        if p.drip then
          p.dy = p.dy + 4.8 * dt
        end
      end
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then
    return
  end
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#1B1B2F")
  local base_a = ca or 255
  for i = 1, #particles do
    local p = particles[i]
    local k = p.life / p.max
    local a = math.floor(base_a * math.min(1, k * 3) * 0.92)
    if a > 0 then
      if p.kind == "splat" then
        ctx:circle(p.x, p.y, p.r, cr, cg, cb, a)
      else
        ctx:circle(p.x, p.y, p.r, cr, cg, cb, a)
        if p.drip and p.dy > 0 then
          ctx:ellipse(p.x, p.y + p.dy * 0.6, p.r * 0.45, p.r * 0.45 + p.dy * 0.4, cr, cg, cb, a)
        end
      end
    end
  end
end
