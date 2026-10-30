-- MouseEffects · Black Hole
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的黑洞：事件视界、光子环、旋转的吸积盘。上游用全屏捕获做引力透镜；
-- 这里用可选的一枚 backdrop_circle 放大镜做局部透镜，失败时退回纯绘制。
-- 注意：开透镜时每帧会抓一次桌面圆区（可用参数关闭）。

name = "Black Hole"
effect_name = "Black Hole"
kind = "cursor"
description = "MouseEffects Black Hole：事件视界 + 吸积盘 + 可选桌面透镜。"
hide_cursor = true

local phase = 0
local cx, cy = 0, 0

function schema()
  return {
    { key = "radius",   label = "整体半径", type = "float", min = 80,  max = 200, step = 10, default = 160 },
    { key = "horizon",  label = "视界比例", type = "float", min = 0.2, max = 0.8, step = 0.01, default = 0.42 },
    { key = "spin",     label = "吸积盘转速", type = "float", min = 0, max = 2,   step = 0.1, default = 0.5 },
    { key = "glow",     label = "辉光强度", type = "float", min = 0,   max = 3,   step = 0.1, default = 1.0 },
    { key = "lens",     label = "桌面透镜", type = "bool",  default = true },
    { key = "diskColor", label = "吸积盘颜色", type = "color", default = "#FF9933" },
  }
end

function init()
  phase = 0
  cx, cy = mouse_x or 0, mouse_y or 0
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.12 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.spin or 0.5)
end

function draw(ctx, params)
  params = params or {}
  local R = params.radius or 160
  local horizon = R * (params.horizon or 0.42)
  local glow = params.glow or 1.0
  local dr, dg, db, da = hex_to_rgb(params.diskColor or "#FF9933")
  local dam = da / 255

  -- 局部透镜：把桌面在 R 圆内放大 1.35 倍；黑盘随后盖住中心。
  local lensed = false
  if params.lens ~= false then
    lensed = ctx:backdrop_circle(cx, cy, R, "magnify", 1.35, 0, 0, 0, 255) == true
  end
  if not lensed then
    -- 抓屏不可用：画一层暗晕，保证仍有"引力井"的层次。
    ctx:radial_stops(cx, cy, R * 1.15, {
      { offset = 0.0, r = 0, g = 0, b = 0, a = 120 },
      { offset = 0.55, r = 0, g = 0, b = 0, a = 45 },
      { offset = 1.0, r = 0, g = 0, b = 0, a = 0 },
    })
  end

  -- 吸积盘：后半圈先画，黑盘盖住中段；前半圈随后画，压在视界下缘。
  local diskN = 44
  local rx = R * 0.95
  local ry = horizon * 1.28
  for half = 0, 1 do
    if half == 1 then
      -- 事件视界 + 光子环夹在两半吸积盘之间。
      ctx:ring(cx, cy, horizon * 1.08, 1.6, 255, 230, 180,
        math.floor(190 * math.min(1.0, glow + 0.3)))
      ctx:circle(cx, cy, horizon, 3, 3, 7, 252)
    end
    for i = 1, diskN do
      local a = (i - 1) * math.pi * 2 / diskN + phase * 1.6
      local front = math.sin(a) > 0
      if (half == 0 and not front) or (half == 1 and front) then
        local wob = 1 + 0.04 * math.sin(a * 3 + phase)
        local x = cx + math.cos(a) * rx * wob
        local y = cy + math.sin(a) * ry * wob
        local k = 0.45 + 0.55 * math.max(0, math.sin(a))
        local alpha = math.floor((half == 0 and 120 or 225) * k * math.min(1.0, glow) * dam)
        ctx:plus_circle(x, y, 1.2 + 2.4 * k, dr, dg, db, alpha)
      end
    end
  end

  -- 视界外的冷色辉光，让黑洞在深色桌面上也有轮廓。
  ctx:ring(cx, cy, horizon * 1.22, 2.4, 90, 130, 255, math.floor(60 * glow))
  ctx:glow(cx, cy, horizon * 0.55, 80, 110, 255, math.floor(55 * glow))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
