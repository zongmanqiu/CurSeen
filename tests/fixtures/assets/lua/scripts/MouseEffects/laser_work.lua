-- MouseEffects · Laser Work
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针移动时向四周发射激光：每次齐射按方向均分，激光边飞边淡出，
-- 在屏幕边缘反弹（上游 "bounce off screen edges"），互撞时在撞点炸开一簇
-- 短命火花（上游 explosionLaserCount/maxCollisionCount）；可选彩虹循环。

name = "Laser Work"
effect_name = "Laser Work"
kind = "trail"
description = "MouseEffects Laser Work：随鼠标移动发射的彩色激光。"

local MAX_LASERS = 100
local lasers = {}
local spawn_acc = 0
local laser_idx = 0

function schema()
  return {
    { key = "rate",    label = "齐射频率", type = "float", min = 1,   max = 30,  step = 1,   default = 12 },
    { key = "rays",    label = "每次方向", type = "int",   min = 1,   max = 8,   step = 1,   default = 4 },
    { key = "length",  label = "激光长度", type = "float", min = 20,  max = 160, step = 5,   default = 70 },
    { key = "width",   label = "激光宽度", type = "float", min = 1,   max = 6,   step = 0.5, default = 2 },
    { key = "speed",   label = "飞行速度", type = "float", min = 100, max = 700, step = 10,  default = 350 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.2 },
    { key = "rainbow", label = "彩虹色",   type = "bool",  default = true },
    { key = "color",   label = "固定颜色", type = "color", default = "#FF3333" },
  }
end

function init()
  lasers = {}
  spawn_acc = 0
  laser_idx = 0
end

function step(dt, t, params)
  params = params or {}
  local speed = math.sqrt((mouse_vx or 0) ^ 2 + (mouse_vy or 0) ^ 2)
  local heading = mouse_heading or 0
  if speed > 1.0 then
    spawn_acc = spawn_acc + (params.rate or 12) * dt
    while spawn_acc >= 1 do
      spawn_acc = spawn_acc - 1
      local rays = math.max(1, math.floor(params.rays or 4))
      local base = heading
      for k = 0, rays - 1 do
        if #lasers >= MAX_LASERS then table.remove(lasers, 1) end
        laser_idx = laser_idx + 1
        local a = base + k * math.pi * 2 / rays + rng(-0.08, 0.08)
        lasers[#lasers + 1] = {
          x = (mouse_x or 0), y = (mouse_y or 0),
          angle = a,
          speed = (params.speed or 350) * rng(0.8, 1.2),
          length = (params.length or 70) * rng(0.75, 1.2),
          width = (params.width or 2) * rng(0.8, 1.2),
          age = 0,
          life = (params.life or 1.2) * rng(0.75, 1.2),
          hue = (laser_idx * 37 + (elapsed or 0) * 90) % 360,
        }
      end
    end
  end

  for i = #lasers, 1, -1 do
    local l = lasers[i]
    l.age = l.age + dt
    if l.age >= l.life then
      table.remove(lasers, i)
    else
      l.x = l.x + math.cos(l.angle) * l.speed * dt
      l.y = l.y + math.sin(l.angle) * l.speed * dt
    end
  end

  -- 屏幕边缘反弹（虚拟桌面边界，上游 LaserWorkEffect.cs）。
  local sx, sy = screen_x or 0, screen_y or 0
  local sw, sh = screen_w or 1920, screen_h or 1080
  for i = 1, #lasers do
    local l = lasers[i]
    if l.x < sx then
      l.x = sx
      l.angle = -l.angle
    elseif l.x > sx + sw then
      l.x = sx + sw
      l.angle = -l.angle
    end
    if l.y < sy then
      l.y = sy
      l.angle = math.pi - l.angle
    elseif l.y > sy + sh then
      l.y = sy + sh
      l.angle = math.pi - l.angle
    end
  end

  -- 互撞爆炸：两束相撞（间距 <12px）即同归于尽，在撞点炸开 6 束短命火花。
  for i = 1, #lasers do
    local a = lasers[i]
    for j = #lasers, i + 1, -1 do
      local b2 = lasers[j]
      local dx, dy = b2.x - a.x, b2.y - a.y
      if dx * dx + dy * dy < 144 then
        for k = 1, 6 do
          if #lasers >= MAX_LASERS then break end
          lasers[#lasers + 1] = {
            x = (a.x + b2.x) * 0.5, y = (a.y + b2.y) * 0.5,
            angle = rng(0, math.pi * 2),
            speed = a.speed * 0.5,
            length = a.length * 0.35,
            width = a.width,
            age = 0,
            life = 0.3,
            hue = (a.hue + b2.hue) * 0.5,
          }
        end
        table.remove(lasers, j)
        table.remove(lasers, i)
        break
      end
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FF3333")
  local am = ca / 255
  for i = 1, #lasers do
    local l = lasers[i]
    local k = 1 - l.age / l.life
    local r, g, b = cr, cg, cb
    if params.rainbow ~= false then
      r, g, b = hsv_to_rgb(l.hue, 1.0, 1.0)
    end
    local tx = l.x - math.cos(l.angle) * l.length
    local ty = l.y - math.sin(l.angle) * l.length
    ctx:line(tx, ty, l.x, l.y, l.width * 3.0, r, g, b, math.floor(50 * k * am))
    ctx:line(tx, ty, l.x, l.y, l.width, r, g, b, math.floor(235 * k * am))
    ctx:plus_circle(l.x, l.y, l.width * 0.9, 255, 255, 255, math.floor(220 * k * am))
  end
end
