-- MouseEffects · DNA Helix
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的双螺旋：两条正弦链竖向排列，中间按等间距画碱基对；整体缓慢旋转。
-- 上游是 400px 高、12 对碱基的 GPU 线框；这里用圆点 + 连线画出同样的骨架。

name = "DNA Helix"
effect_name = "DNA Helix"
kind = "cursor"
description = "MouseEffects DNA Helix：跟随光标的双螺旋。"
hide_cursor = true

local cx, cy = 0, 0
local phase = 0

function schema()
  return {
    { key = "height",  label = "螺旋高度", type = "float", min = 120, max = 400, step = 10, default = 300 },
    { key = "radius",  label = "螺旋半径", type = "float", min = 20,  max = 80,  step = 2,  default = 50 },
    { key = "twist",   label = "扭转密度", type = "float", min = 0.02, max = 0.12, step = 0.01, default = 0.05 },
    { key = "spin",    label = "旋转速度", type = "float", min = 0,   max = 3,   step = 0.1, default = 1.0 },
    { key = "pairs",   label = "碱基对数", type = "int",   min = 6,   max = 20,  step = 1,  default = 12 },
    { key = "width",   label = "链宽",     type = "float", min = 1,   max = 5,   step = 0.5, default = 2 },
    { key = "color1",  label = "链一颜色", type = "color", default = "#4169E1" },
    { key = "color2",  label = "链二颜色", type = "color", default = "#DC143C" },
  }
end

function init()
  cx, cy = mouse_x or 0, mouse_y or 0
  phase = 0
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.16 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.spin or 1.0) * 1.6
end

function draw(ctx, params)
  params = params or {}
  local H = params.height or 300
  local R = params.radius or 50
  local twist = params.twist or 0.05
  local pairsN = math.max(6, math.min(20, math.floor(params.pairs or 12)))
  local width = params.width or 2
  local c1r, c1g, c1b, c1a = hex_to_rgb(params.color1 or "#4169E1")
  local c2r, c2g, c2b, c2a = hex_to_rgb(params.color2 or "#DC143C")
  local am1, am2 = c1a / 255, c2a / 255

  local samples = 36
  local prev1 = nil
  local prev2 = nil
  local pairSet = {}
  for i = 1, pairsN do
    pairSet[math.floor((i - 0.5) * samples / pairsN) + 1] = true
  end

  for i = 0, samples do
    local u = i / samples
    local y = cy - H * 0.5 + H * u
    local a = phase + u * H * twist * 0.09
    local x1 = cx + math.sin(a) * R
    local x2 = cx - math.sin(a) * R
    if prev1 then
      ctx:line(prev1[1], prev1[2], x1, y, width, c1r, c1g, c1b, math.floor(200 * am1))
      ctx:line(prev2[1], prev2[2], x2, y, width, c2r, c2g, c2b, math.floor(200 * am2))
    end
    prev1, prev2 = { x1, y }, { x2, y }
    ctx:plus_circle(x1, y, width * 1.05, c1r, c1g, c1b, math.floor(230 * am1))
    ctx:plus_circle(x2, y, width * 1.05, c2r, c2g, c2b, math.floor(230 * am2))
    if pairSet[i + 1] then
      ctx:line(x1, y, x2, y, width * 0.7,
        math.floor((c1r + c2r) * 0.5), math.floor((c1g + c2g) * 0.5), math.floor((c1b + c2b) * 0.5),
        math.floor(150 * (am1 + am2) * 0.5))
    end
  end

  ctx:glow(cx, cy, R * 0.9, c1r, c1g, c1b, math.floor(35 * am1))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
