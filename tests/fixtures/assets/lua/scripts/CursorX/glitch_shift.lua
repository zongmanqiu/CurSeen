-- CursorX · Glitch Shift
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：白色主点 + 红/绿/蓝三个加色通道点。平时三色轻微错位，每隔
-- glitchInterval 撕裂一小段；点击触发一次长而大的故障爆闪。指针停在交互提示上时白点变大。
-- 上游用 mix-blend-mode:screen，这里用加色图元（Blend::Plus）近似。

name = "Glitch Shift"
effect_name = "Glitch Shift"
kind = "cursor"
description = "CursorX Glitch Shift：RGB 通道撕裂故障光标，点击大爆闪。"
hide_cursor = true

local current_size = 13
local glitch = false
local glitch_start = 0
local last_periodic = 0
local split_boost = 1.0
local phase = 0

function schema()
  return {
    { key = "split",           label = "通道错位", type = "int",   min = 1,   max = 20,  step = 1,    default = 5 },
    { key = "glitchInterval",  label = "故障间隔(ms)", type = "int",   min = 200, max = 2000,step = 50,   default = 500 },
    { key = "burstDuration",   label = "持续时间(ms)", type = "int",   min = 100, max = 1000,step = 50,   default = 300 },
    { key = "pointerAnim",     label = "悬停放大", type = "bool",  default = true },
    { key = "pointerSizeMult", label = "悬停倍数", type = "float", min = 1.0, max = 3.0, step = 0.1,  default = 1.7 },
    { key = "clickAnim",       label = "点击爆闪", type = "bool",  default = true },
    { key = "clickSplitMult",  label = "爆闪倍数", type = "float", min = 1.5, max = 8.0, step = 0.1,  default = 4.4 },
  }
end

function init()
  current_size = 13
  glitch = false
  glitch_start = 0
  last_periodic = 0
  split_boost = 1.0
  phase = 0
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  glitch = true
  glitch_start = wall_ms or 0
  split_boost = params.clickSplitMult or 4.4
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local now = wall_ms or 0
  local interval = params.glitchInterval or 500
  local duration = params.burstDuration or 300

  if glitch then
    -- 点击爆闪固定 250ms（与上游一致）；周期故障用参数时长。
    local limit = split_boost > 1.0 and 250 or duration
    if now - glitch_start > limit then
      glitch = false
      split_boost = 1.0
      last_periodic = now
    end
  elseif now - last_periodic >= interval then
    glitch = true
    glitch_start = now
    last_periodic = now
    split_boost = 1.0
  end

  phase = phase + dt * 33.0
  local target = 13 * ((cursor_hover and params.pointerAnim ~= false)
    and (params.pointerSizeMult or 1.7) or 1.0)
  current_size = current_size + (target - current_size) * math.min(1.0, 0.15 * f)
end

function draw(ctx, params)
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  local split = (params.split or 5) * (glitch and split_boost or 1.0)
  local half = current_size * 0.5

  local rx, ry, gx, gy, bx, by
  if glitch then
    rx = mx + math.sin(phase) * split * 3.0
    ry = my + math.cos(phase * 1.2) * split
    gx = mx + math.sin(phase + 2.1) * split * 2.0
    gy = my + 2
    bx = mx + math.sin(phase + 4.2) * split * 2.5
    by = my - 2
  else
    rx, ry = mx - split, my - 1
    gx, gy = mx, my
    bx, by = mx + split, my + 1
  end

  ctx:plus_circle(rx, ry, half, 255, 0, 0, 210)
  ctx:plus_circle(gx, gy, half, 0, 255, 136, 190)
  ctx:plus_circle(bx, by, half, 0, 85, 255, 190)
  ctx:circle(mx, my, half, 255, 255, 255, 255)
  ctx:circle(mx, my, half * 0.35, 0, 0, 0, 90)
end
