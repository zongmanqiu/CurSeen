-- CursorFX · birds
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：一群鸟从按下点起飞、扇翅并四散。翅膀用两条二次贝塞尔曲线各采样成折线，
-- 朝向按速度自己旋转点；没有文字/图片依赖，粒子上限 180。

name = "birds"
effect_name = "birds"
kind = "click"
description = "CursorFX birds：从按下点起飞的鸟群。"

local MAX_PARTICLES = 200
local particles = {}
local WING_STEPS = 6

function schema()
  return {
    { key = "color",   label = "单色",   type = "color", alpha = true, default = "#F4F6FF" },
    { key = "rainbow", label = "彩虹色", type = "bool",  default = true },
    { key = "count",   label = "鸟数",   type = "int",   min = 1,  max = 24, step = 1, default = 9 },
    { key = "size",    label = "大小",   type = "float", min = 5,  max = 18, step = 1, default = 9 },
  }
end

function init()
  particles = {}
end

function on_press(x, y, t, params)
  local count = math.floor(params.count or 9)
  local base = rng(-2.6, -0.5)
  local scale = params.size or 9
  for _ = 1, count do
    if #particles >= MAX_PARTICLES then
      break
    end
    local a = base + rng(-0.5, 0.5)
    local sp = rng(2.5, 4.5) * 60
    local life = rng(2.0, 3.3)
    particles[#particles + 1] = {
      x = x + rng(-10, 10),
      y = y + rng(-10, 10),
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp,
      flap = rng(0, 6),
      fs = rng(0.35, 0.5),
      size = scale * rng(0.65, 1.35),
      life = life,
      max = life,
      hue = rng(0, 360),
    }
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
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.flap = p.flap + p.fs * dt * 60
      p.vy = p.vy + math.sin(p.flap) * 42 * dt
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then
    return
  end
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#F4F6FF")
  local base_a = ca or 255
  for i = 1, #particles do
    local p = particles[i]
    local k = p.life / p.max
    local a = math.floor(base_a * math.min(1, k * 3))
    local r, g, b = cr, cg, cb
    if params.rainbow ~= false then
      r, g, b = hsv_to_rgb((p.hue + (elapsed or 0) * 30.0) % 360.0, 0.85, 1.0)
    end
    if a > 0 and p.size > 0.5 then
      local ang = math.atan(p.vy, p.vx)
      local co, si = math.cos(ang), math.sin(ang)
      local w = math.sin(p.flap) * p.size * 0.9
      local lx1, ly1 = -p.size, -w
      local lcx, lcy = -p.size * 0.4, 0
      local mx, my = 0, 0
      local rcx, rcy = p.size * 0.4, 0
      local rx1, ry1 = p.size, -w
      local prev_x, prev_y

      local function emit(px, py)
        local wx = p.x + px * co - py * si
        local wy = p.y + px * si + py * co
        if prev_x then
          ctx:line(prev_x, prev_y, wx, wy, math.max(1.2, p.size * 0.18), r, g, b, a)
        end
        prev_x, prev_y = wx, wy
      end

      for s = 0, WING_STEPS - 1 do
        local u = s / WING_STEPS
        local iu = 1 - u
        emit(
          iu * iu * lx1 + 2 * iu * u * lcx + u * u * mx,
          iu * iu * ly1 + 2 * iu * u * lcy + u * u * my
        )
      end
      for s = 1, WING_STEPS do
        local u = s / WING_STEPS
        local iu = 1 - u
        emit(
          iu * iu * mx + 2 * iu * u * rcx + u * u * rx1,
          iu * iu * my + 2 * iu * u * rcy + u * u * ry1
        )
      end
    end
  end
end
