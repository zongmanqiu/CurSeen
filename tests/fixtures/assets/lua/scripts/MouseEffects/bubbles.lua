-- MouseEffects · Bubbles
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处升起的肥皂泡：透明填充 + 彩色边 + 左上高光；左键把气泡戳破成小水珠。
-- 上游可选屏幕折射；CurSeen 没有逐泡折射采样，这里保留高光/彩边版本。

name = "Bubbles"
effect_name = "Bubbles"
kind = "cursor"
description = "MouseEffects Bubbles：从光标升起、可戳破的肥皂泡。"
hide_cursor = true

local MAX_BUBBLES = 90
local bubbles = {}
local drops = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",    label = "生成频率", type = "float", min = 2,   max = 30,  step = 1,   default = 14 },
    { key = "size",    label = "气泡大小", type = "float", min = 6,   max = 32,  step = 1,   default = 16 },
    { key = "rise",    label = "上浮速度", type = "float", min = 20,  max = 120, step = 5,   default = 50 },
    { key = "wobble",  label = "左右摆动", type = "float", min = 0,   max = 60,  step = 5,   default = 20 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.2, max = 1,   step = 0.05, default = 0.9 },
    { key = "pop",     label = "点击戳破", type = "bool",  default = true },
    { key = "color",   label = "气泡颜色", type = "color", default = "#78C8FF" },
  }
end

function init()
  bubbles = {}
  drops = {}
  spawn_acc = 0
end

local function spawn_bubble(x, y, params)
  if #bubbles >= MAX_BUBBLES then return end
  local base = params.size or 16
  local r = base * rng(0.55, 1.25)
  bubbles[#bubbles + 1] = {
    x = x + rng(-12, 12),
    y = y + rng(-6, 6),
    r = r,
    vx = rng(-12, 12),
    vy = -(params.rise or 50) * rng(0.7, 1.3),
    phase = rng(0, math.pi * 2),
    life = rng(2.0, 4.5),
    age = 0,
  }
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.pop == false then return end
  for i = 1, #bubbles do
    local b = bubbles[i]
    for _ = 1, 4 do
      local a = rng(0, math.pi * 2)
      local sp = rng(30, 90)
      drops[#drops + 1] = {
        x = b.x, y = b.y,
        vx = math.cos(a) * sp,
        vy = math.sin(a) * sp,
        r = rng(1.0, 2.4),
        life = 0.5,
        age = 0,
      }
    end
  end
  bubbles = {}
end

function step(dt, t, params)
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  spawn_acc = spawn_acc + (params.rate or 14) * dt
  while spawn_acc >= 1 do
    spawn_acc = spawn_acc - 1
    spawn_bubble(mx, my, params)
  end

  local wob = params.wobble or 20
  for i = #bubbles, 1, -1 do
    local b = bubbles[i]
    b.age = b.age + dt
    b.phase = b.phase + dt * 2.2
    b.x = b.x + (b.vx + math.sin(b.phase) * wob) * dt
    b.y = b.y + b.vy * dt
    if b.age >= b.life or b.y < -60 then
      table.remove(bubbles, i)
    end
  end

  for i = #drops, 1, -1 do
    local d = drops[i]
    d.age = d.age + dt
    d.x = d.x + d.vx * dt
    d.y = d.y + d.vy * dt
    d.vy = d.vy + 160 * dt
    if d.age >= d.life then table.remove(drops, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local op = params.opacity or 0.9
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#78C8FF")
  local am = ca / 255

  for i = 1, #bubbles do
    local b = bubbles[i]
    local fade = math.min(1.0, b.age * 2.0) * math.max(0.0, 1.0 - b.age / b.life)
    local a = math.floor(255 * op * fade * am)
    ctx:circle(b.x, b.y, b.r, cr, cg, cb, math.floor(a * 0.10))
    ctx:ring(b.x, b.y, b.r, 1.6, cr, cg, cb, math.floor(a * 0.85))
    ctx:ring(b.x, b.y, b.r * 0.92, 0.8, 255, 255, 255, math.floor(a * 0.22))
    ctx:plus_circle(b.x - b.r * 0.32, b.y - b.r * 0.34, b.r * 0.16, 255, 255, 255,
      math.floor(a * 0.85))
  end

  for i = 1, #drops do
    local d = drops[i]
    local a = math.floor(255 * (1.0 - d.age / d.life) * am)
    ctx:plus_circle(d.x, d.y, d.r, cr, cg, cb, a)
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
