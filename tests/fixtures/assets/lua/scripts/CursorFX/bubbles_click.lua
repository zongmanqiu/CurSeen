-- CursorFX · bubbles（click 版）
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：按下点冒出一团肥皂泡，向上漂、左右轻摆，寿命将尽时“啵”地炸成一个小白环。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。
-- ★ 与同目录的 bubbles.lua（trail 版）是两个独立脚本，本文件专供点击宿主。

name = "bubbles"
effect_name = "bubbles"
kind = "click"
description = "CursorFX bubbles（click）：上浮、轻摆并逐个破裂的肥皂泡。"

local MAX_BUBBLES = 200
local bubbles = {}

function schema()
  return {
    { key = "color", label = "颜色",     type = "color", alpha = true, default = "#78C8FF" },
    { key = "count", label = "气泡数",   type = "int",   min = 4,  max = 40, step = 1,   default = 18 },
    { key = "size",  label = "气泡大小", type = "float", min = 3,  max = 20, step = 0.5, default = 9 },
    { key = "life",  label = "寿命倍率", type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
  }
end

function init()
  bubbles = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

function on_press(x, y, t, params)
  local n = math.floor(params.count or 18)
  if n < 1 then n = 1 elseif n > 40 then n = 40 end
  local size = params.size or 9
  local life_k = params.life or 1.0

  for i = 1, n do
    local ang = rng(0, 6.2831853)
    local sp = rng(60, 240)
    local life = rng(0.85, 1.85) * life_k
    bubbles[#bubbles + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp - 60,
      r = size * rng(0.45, 1.5),
      ph = rng(0, 6.2831853),
      life = life,
      max = life,
      popped = false,
    }
  end

  while #bubbles > MAX_BUBBLES do
    table.remove(bubbles, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local now = t or elapsed or 0
  local damp = (0.95) ^ (ds * 60.0)
  local n = #bubbles
  local w = 0
  for i = 1, n do
    local p = bubbles[i]
    p.vx = p.vx * damp
    p.vy = p.vy * damp - 72.0 * ds
    p.x = p.x + (p.vx + math.sin(now * 3.0 + p.ph) * 24.0) * ds
    p.y = p.y + p.vy * ds
    p.life = p.life - ds
    if p.life < 0.13 and not p.popped then
      p.popped = true
    end
    if p.life > 0 then
      w = w + 1
      bubbles[w] = p
    end
  end
  for i = #bubbles, w + 1, -1 do
    bubbles[i] = nil
  end
end

function draw(ctx, params)
  if #bubbles == 0 then return end

  local r, g, b, a = hex_to_rgb(params.color or "#78C8FF")
  for i = 1, #bubbles do
    local p = bubbles[i]
    if p.popped then
      -- 破裂：原版是“寿命最后 8 帧画一个迅速撑大的白环”，这里按剩余 0.13s 折算。
      local u = 1.0 - p.life / 0.13
      if u < 0 then u = 0 elseif u > 1 then u = 1 end
      local al = a8(255, (1.0 - u) * 0.85)
      if al > 0 then
        ctx:ring(p.x, p.y, p.r * (1.0 + u * 8.0), 1, 255, 255, 255, al)
      end
    else
      local fill_a = a8(a, 0.18)
      local rim_a = a8(255, 0.75)
      if fill_a > 0 then
        ctx:circle(p.x, p.y, p.r, r, g, b, fill_a)
      end
      ctx:ring(p.x, p.y, p.r, 1, 255, 255, 255, rim_a)
      -- 左上高光：一颗小实心圆，肥皂泡的“湿感”主要来自它。
      ctx:circle(p.x - p.r * 0.35, p.y - p.r * 0.35, p.r * 0.2, 255, 255, 255, a8(255, 0.85))
    end
  end
end
