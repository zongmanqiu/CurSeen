-- CursorX · Crosshair Scope
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：瞄准镜圆环 + 四向刻度 + 四角括号 + 旋转扫描扇区；指针停在交互提示上
-- 时切换为锁定色、扫描加快；点击时整镜挤压一次再弹回。

name = "Crosshair Scope"
effect_name = "Crosshair Scope"
kind = "cursor"
description = "CursorX Crosshair Scope：带扫描扇区的瞄准镜光标。"
hide_cursor = true

local TAU = math.pi * 2
local scan = 0
local scale_t = 1.0
local scale_c = 1.0
local squeeze_start = -1

function schema()
  return {
    { key = "color",           label = "常态颜色", type = "color", alpha = true, default = "#5CF4FC" },
    { key = "lockedColor",     label = "锁定颜色", type = "color", alpha = true, default = "#FF4455" },
    { key = "radius",          label = "瞄准半径", type = "int",   min = 20,  max = 80,  step = 2,    default = 40 },
    { key = "scanSpeed",       label = "扫描速度", type = "float", min = 0.01,max = 0.12,step = 0.005,default = 0.045 },
    { key = "pointerAnim",     label = "悬停锁定", type = "bool",  default = true },
    { key = "pointerSpeedMult",label = "扫描加速", type = "float", min = 1.0, max = 3.0, step = 0.1,  default = 2.0 },
    { key = "clickAnim",       label = "点击挤压", type = "bool",  default = true },
    { key = "clickSqueeze",    label = "挤压幅度", type = "float", min = 0.4, max = 0.95,step = 0.05, default = 0.75 },
  }
end

function init()
  scan = 0
  scale_t = 1.0
  scale_c = 1.0
  squeeze_start = -1
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then
    scale_t = params.clickSqueeze or 0.75
    squeeze_start = wall_ms or 0
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local hover = cursor_hover and params.pointerAnim ~= false
  scan = scan + (params.scanSpeed or 0.045) * (hover and (params.pointerSpeedMult or 2.0) or 1.0) * f
  scale_c = scale_c + (scale_t - scale_c) * math.min(1.0, 0.2 * f)
  -- 挤压保持 200ms 再回弹（用墙钟计时，不依赖帧率）。
  if squeeze_start >= 0 and (wall_ms or 0) - squeeze_start > 200 then
    scale_t = 1.0
    squeeze_start = -1
  end
end

local function sector(ctx, x, y, r, start_a, end_a, cr, cg, cb, a)
  local steps = 18
  local poly = { { x, y } }
  for i = 0, steps do
    local ang = start_a + (end_a - start_a) * i / steps
    poly[#poly + 1] = { x + math.cos(ang) * r, y + math.sin(ang) * r }
  end
  ctx:polygon(poly, cr, cg, cb, a)
end

function draw(ctx, params)
  params = params or {}
  local hover = cursor_hover and params.pointerAnim ~= false
  local cr, cg, cb, ca
  if hover then
    cr, cg, cb, ca = hex_to_rgb(params.lockedColor or "#FF4455")
  else
    cr, cg, cb, ca = hex_to_rgb(params.color or "#5CF4FC")
  end

  local s = scale_c
  local r = (params.radius or 40) * s
  local tick = 14 * s
  local mx, my = mouse_x or 0, mouse_y or 0

  ctx:ring(mx, my, r + 2.0, 4.0, cr, cg, cb, math.floor(ca * 0.16))
  ctx:ring(mx, my, r, 1.5, cr, cg, cb, ca)

  -- 四向刻度
  local dirs = { { 1, 0 }, { -1, 0 }, { 0, 1 }, { 0, -1 } }
  for i = 1, 4 do
    local dx, dy = dirs[i][1], dirs[i][2]
    ctx:line(mx + dx * (r + 2), my + dy * (r + 2),
      mx + dx * (r + tick), my + dy * (r + tick), 1.5, cr, cg, cb, ca)
  end
  ctx:circle(mx, my, 2.5 * s, cr, cg, cb, ca)

  -- 扫描扇区（半透明填充）
  sector(ctx, mx, my, r - 6 * s, scan, scan + math.pi * 0.38, cr, cg, cb, math.floor(ca * 0.28))

  -- 四角括号
  local off = r + 18 * s
  local bs = 14 * s
  for i = 1, 4 do
    local sx = (i == 1 or i == 3) and -1 or 1
    local sy = (i <= 2) and -1 or 1
    local x = mx + sx * off
    local y = my + sy * off
    ctx:line(x, y, x + sx * bs, y, 1.4, cr, cg, cb, ca)
    ctx:line(x, y, x, y + sy * bs, 1.4, cr, cg, cb, ca)
  end
end
