-- MouseEffects · Zoom
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的圆形放大镜：用 ctx:backdrop_circle 采样桌面并放大，外加镜圈与十字。
-- 上游支持圆形/矩形两种镜片；CurSeen 目前只有圆形桌面采样，矩形模式未纳入。

name = "Zoom"
effect_name = "Zoom"
kind = "cursor"
description = "MouseEffects Zoom：跟随指针的圆形桌面放大镜。"
hide_cursor = true

local cx, cy = 0, 0

function schema()
  return {
    { key = "radius",      label = "镜片半径", type = "float", min = 60,  max = 200, step = 5,   default = 140 },
    { key = "zoom",        label = "放大倍率", type = "float", min = 1.2, max = 4.0, step = 0.1, default = 2.0 },
    { key = "opacity",     label = "镜片不透明", type = "float", min = 0.2, max = 1, step = 0.05, default = 1.0 },
    { key = "borderWidth", label = "镜圈宽度", type = "float", min = 0,   max = 8,   step = 0.5, default = 2.0 },
    { key = "crosshair",   label = "十字准线", type = "bool",  default = true },
    { key = "borderColor", label = "镜圈颜色", type = "color", default = "#3399FF" },
  }
end

function init()
  cx, cy = mouse_x or 0, mouse_y or 0
end

function step(dt, t, params)
  cx = mouse_x or cx
  cy = mouse_y or cy
end

function draw(ctx, params)
  params = params or {}
  local R = math.min(200, params.radius or 140)
  local zoom = params.zoom or 2.0
  local opacity = params.opacity or 1.0
  local bw = params.borderWidth or 2.0
  local cr, cg, cb, ca = hex_to_rgb(params.borderColor or "#3399FF")
  local am = ca / 255

  local lensed = ctx:backdrop_circle(cx, cy, R, "magnify", zoom, 0, 0, 0,
    math.floor(255 * opacity)) == true
  if not lensed then
    -- 抓屏不可用：给出一个玻璃色圆片，至少仍能看到镜片位置。
    ctx:circle(cx, cy, R, 60, 110, 180, math.floor(60 * opacity))
  end

  if bw > 0.01 then
    ctx:ring(cx, cy, R, bw, cr, cg, cb, math.floor(225 * am))
    ctx:ring(cx, cy, R - bw * 1.4, 0.8, 255, 255, 255, math.floor(90 * am))
  end

  if params.crosshair ~= false then
    local s = R * 0.72
    ctx:line(cx - s, cy, cx + s, cy, 0.8, cr, cg, cb, math.floor(110 * am))
    ctx:line(cx, cy - s, cx, cy + s, 0.8, cr, cg, cb, math.floor(110 * am))
    ctx:plus_circle(cx, cy, 1.6, 255, 255, 255, math.floor(180 * am))
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
