-- CursorX · Difference Blend
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：大小两颗球以“差值”方式取桌面颜色（|桌面 − 球色|），在任何背景上
-- 都会反色，像一块挖空的透视镜。外球滞后、内球更快；悬停时外球放大、内球缩小，
-- 点击弹一下。
--
-- ★ 需要宿主 `ctx:backdrop_circle`（真桌面采样）。抓屏不可用时退回白球 + 深色描边，
--   并写一条日志；不会画出“套娃镜片”。

name = "Difference Blend"
effect_name = "Difference Blend"
kind = "cursor"
description = "CursorX Difference Blend：双球差值反色光标。"
hide_cursor = true

local bx, by = 0, 0
local sx, sy = 0, 0
local big_scale = 1.0
local small_scale = 1.0
local click_t = -1
local warned = false

function schema()
  return {
    { key = "color",       label = "球体颜色", type = "color", alpha = true, default = "#F7F8FA" },
    { key = "bigSize",     label = "外球大小", type = "int",   min = 10,  max = 50,  step = 1,    default = 25 },
    { key = "smallSize",   label = "内球大小", type = "int",   min = 2,   max = 16,  step = 1,    default = 6 },
    { key = "bigSpeed",    label = "外球滞后", type = "float", min = 0.02,max = 0.4, step = 0.01, default = 0.10 },
    { key = "smallSpeed",  label = "内球滞后", type = "float", min = 0.05,max = 0.8, step = 0.01, default = 0.25 },
    { key = "pointerAnim", label = "悬停缩放", type = "bool",  default = true },
    { key = "pointerScale",label = "外球倍数", type = "float", min = 1.2, max = 4.5, step = 0.1,  default = 2.5 },
    { key = "clickAnim",   label = "点击脉冲", type = "bool",  default = true },
    { key = "clickScale",  label = "脉冲幅度", type = "float", min = 0.2, max = 2.0, step = 0.1,  default = 1.0 },
  }
end

function init()
  bx, by = mouse_x or 0, mouse_y or 0
  sx, sy = bx, by
  big_scale = 1.0
  small_scale = 1.0
  click_t = -1
  warned = false
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or bx, mouse_y or by

  local bk = 1.0 - math.pow(1.0 - (params.bigSpeed or 0.10), f)
  local sk = 1.0 - math.pow(1.0 - (params.smallSpeed or 0.25), f)
  bx = bx + (mx - bx) * bk
  by = by + (my - by) * bk
  sx = sx + (mx - sx) * sk
  sy = sy + (my - sy) * sk

  local target_big, target_small = 1.0, 1.0
  if cursor_hover and params.pointerAnim ~= false then
    target_big = params.pointerScale or 2.5
    target_small = 0.5
  end
  if click_t >= 0 and params.clickAnim ~= false then
    local bounce = math.sin(click_t * math.pi)
    local cs = params.clickScale or 1.0
    target_big = target_big * (1 - bounce * cs)
    target_small = target_small * (1 + bounce * cs * 1.5)
    click_t = click_t + dt * 4.8
    if click_t >= 1 then click_t = -1 end
  end
  big_scale = big_scale + (target_big - big_scale) * math.min(1.0, 0.15 * f)
  small_scale = small_scale + (target_small - small_scale) * math.min(1.0, 0.15 * f)
end

local function ball(ctx, x, y, radius, r, g, b, alpha)
  if radius < 0.5 then return end
  local ok = ctx:backdrop_circle(x, y, radius, "difference", 1.0, r, g, b, alpha)
  if ok then return end
  -- 采不到桌面：画双色实心球，仍然能看见指针（不伪装成反色）。
  ctx:circle(x, y, radius, r, g, b, math.floor(alpha * 235 / 255))
  ctx:ring(x, y, radius, math.max(1.2, radius * 0.12), 0, 0, 0,
    math.floor(alpha * 170 / 255))
  if not warned then
    warned = true
    if ext and ext.log then ext:log("Difference Blend: 桌面采样不可用，已退回静态双色球") end
  end
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#F7F8FA")
  ctx:layer("top")
  ball(ctx, bx, by, (params.bigSize or 25) * 0.5 * big_scale, r, g, b, ca)
  ball(ctx, sx, sy, (params.smallSize or 6) * 0.5 * small_scale, r, g, b, ca)
end
