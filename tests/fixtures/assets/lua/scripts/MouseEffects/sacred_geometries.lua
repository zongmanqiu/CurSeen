-- MouseEffects · Sacred Geometries
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的神圣几何：生命之花、梅塔特隆立方、Sri Yantra 三种图案，缓慢旋转。
-- 上游是 GPU 线框；这里用 ctx:ring / ctx:line / ctx:polygon 画同样的几何骨架。

name = "Sacred Geometries"
effect_name = "Sacred Geometries"
kind = "cursor"
description = "MouseEffects Sacred Geometries：生命之花 / 梅塔特隆 / Sri Yantra。"
hide_cursor = true

local cx, cy = 0, 0
local rot = 0

function schema()
  return {
    { key = "pattern",   label = "图案",     type = "choice", choices = { "flower", "metatron", "sri" },
      labels = { "生命之花", "梅塔特隆", "Sri Yantra" }, default = "flower" },
    { key = "radius",    label = "半径",     type = "float", min = 70,  max = 260, step = 5,  default = 140 },
    { key = "spin",      label = "旋转速度(度/秒)", type = "float", min = 0,   max = 60,  step = 2,  default = 12 },
    { key = "lineWidth", label = "线宽",     type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.2 },
    { key = "glow",      label = "辉光强度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.8 },
    { key = "opacity",   label = "不透明度", type = "float", min = 0.1, max = 1,   step = 0.05, default = 0.8 },
    { key = "color1",    label = "主色",     type = "color", default = "#CC80FF" },
    { key = "color2",    label = "辅色",     type = "color", default = "#66CCFF" },
  }
end

function init()
  cx, cy = mouse_x or 0, mouse_y or 0
  rot = 0
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.14 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  rot = rot + dt * (params.spin or 12) * math.pi / 180.0
end

local function pt(r, a)
  return cx + math.cos(a + rot) * r, cy + math.sin(a + rot) * r
end

local function ring(r, w, cr, cg, cb, a)
  ctx:ring(cx, cy, r, w, cr, cg, cb, a)
end

local function line_pt(r1, a1, r2, a2, w, cr, cg, cb, a)
  local x1, y1 = pt(r1, a1)
  local x2, y2 = pt(r2, a2)
  ctx:line(x1, y1, x2, y2, w, cr, cg, cb, a)
end

local function draw_flower(ctx, R, w, alpha, c1, c2)
  local d = R * 0.33
  local cr, cg, cb, ca = hex_to_rgb(c1)
  local sr, sg, sb, sa = hex_to_rgb(c2)
  local am1, am2 = ca / 255, sa / 255
  local centers = { { 0, 0 } }
  for k = 0, 5 do
    centers[#centers + 1] = { d, k * math.pi / 3 }
  end
  for k = 0, 5 do
    centers[#centers + 1] = { d * 2, k * math.pi / 3 }
    centers[#centers + 1] = { d * math.sqrt(3), k * math.pi / 3 + math.pi / 6 }
  end
  local rr = d * 0.62
  for i, c in ipairs(centers) do
    local x, y = pt(c[1], c[2])
    local r0, g0, b0 = cr, cg, cb
    local am = am1
    if i % 3 == 0 then r0, g0, b0 = sr, sg, sb; am = am2 end
    ctx:ring(x, y, rr, w, r0, g0, b0, math.floor(alpha * am))
  end
  ring(R, w, sr, sg, sb, math.floor(alpha * 0.8 * am2))
  ring(R * 1.04, w * 0.8, cr, cg, cb, math.floor(alpha * 0.45 * am1))
end

local function draw_metatron(ctx, R, w, alpha, c1, c2)
  local d = R * 0.34
  local cr, cg, cb, ca = hex_to_rgb(c1)
  local sr, sg, sb, sa = hex_to_rgb(c2)
  local am1, am2 = ca / 255, sa / 255
  local centers = { { 0, 0 } }
  for k = 0, 5 do
    centers[#centers + 1] = { d, k * math.pi / 3 }
  end
  for k = 0, 5 do
    centers[#centers + 1] = { d * 2, k * math.pi / 3 + math.pi / 6 }
  end
  for i = 1, #centers do
    local x, y = pt(centers[i][1], centers[i][2])
    ctx:ring(x, y, R * 0.17, w, sr, sg, sb, math.floor(alpha * am2))
    for j = i + 1, #centers do
      local x2, y2 = pt(centers[j][1], centers[j][2])
      ctx:line(x, y, x2, y2, w * 0.6, cr, cg, cb, math.floor(alpha * 0.35 * am1))
    end
  end
  ring(R, w, cr, cg, cb, math.floor(alpha * 0.85 * am1))
end

local function draw_sri(ctx, R, w, alpha, c1, c2)
  local cr, cg, cb, ca = hex_to_rgb(c1)
  local sr, sg, sb, sa = hex_to_rgb(c2)
  local am1, am2 = ca / 255, sa / 255
  -- 9 个交叠三角形：4 上 5 下，尺寸与偏心逐层变化。
  for i = 1, 9 do
    local up = (i % 2 == 1)
    local k = i / 9
    local half = R * (0.30 + 0.62 * k)
    local h = R * (0.55 + 0.34 * k)
    local yoff = (up and -1 or 1) * R * 0.05 * (i - 5)
    local ax = 0
    local ay = yoff + (up and h * 0.45 or -h * 0.45)
    local bx = -half
    local by = yoff + (up and -h * 0.55 or h * 0.55)
    local ex = half
    local ey = by
    -- 旋转后按局部坐标平移；pt() 只处理半径/角度，这里直接算旋转点。
    local ca, sa = math.cos(rot), math.sin(rot)
    local function rp(x, y) return cx + x * ca - y * sa, cy + x * sa + y * ca end
    local p1x, p1y = rp(ax, ay)
    local p2x, p2y = rp(bx, by)
    local p3x, p3y = rp(ex, ey)
    ctx:line(p1x, p1y, p2x, p2y, w, cr, cg, cb, math.floor(alpha * am1))
    ctx:line(p2x, p2y, p3x, p3y, w, cr, cg, cb, math.floor(alpha * am1))
    ctx:line(p3x, p3y, p1x, p1y, w, cr, cg, cb, math.floor(alpha * am1))
  end
  -- 外圈 + 16 瓣莲花（用短弧线近似）。
  ring(R, w, sr, sg, sb, math.floor(alpha * 0.85 * am2))
  for k = 0, 15 do
    local a = k * math.pi / 8
    line_pt(R * 0.92, a - 0.06, R * 1.02, a, w * 0.8, sr, sg, sb, math.floor(alpha * 0.6 * am2))
    line_pt(R * 1.02, a, R * 0.92, a + 0.06, w * 0.8, sr, sg, sb, math.floor(alpha * 0.6 * am2))
  end
end

function draw(ctx, params)
  params = params or {}
  local R = params.radius or 140
  local w = params.lineWidth or 1.2
  local alpha = math.floor(255 * (params.opacity or 0.8))
  local glow = params.glow or 0.8
  local pattern = params.pattern or "flower"
  local c1 = params.color1 or "#CC80FF"
  local c2 = params.color2 or "#66CCFF"

  if pattern == "metatron" then
    draw_metatron(ctx, R, w, alpha, c1, c2)
  elseif pattern == "sri" then
    draw_sri(ctx, R, w, alpha, c1, c2)
  else
    draw_flower(ctx, R, w, alpha, c1, c2)
  end

  if glow > 0.01 then
    local gr, gg, gb, ga = hex_to_rgb(c2)
    ctx:glow(cx, cy, R * 0.45, gr, gg, gb, math.floor(45 * glow * ga / 255))
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
