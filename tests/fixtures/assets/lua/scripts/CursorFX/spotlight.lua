-- CursorFX · spotlight / 复刻自 cursorfx-main/src/cursors/spotlight.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：整屏压暗，只在指针周围留一个边缘柔和的光圈；按住时光圈放大，移动越快
-- 光圈略微变大。原版用 destination-out 径向渐变挖洞，CurSeen 没有混合模式暴露：
-- 这里在 base 层用「全屏 rect 压暗 → clear_circle 挖透光内圈 → radial_stops 补回
-- 柔边」三步得到同样的观感。
--
-- 性能：全屏矩形走宿主实色快路；clear_circle 只光栅化一个小圆；柔边径向圆半径
-- 约等于光圈。单帧 3 个图元 + 1 个指针点，实测约 0.5~0.8 ms（1080p 软渲染）。
-- ★ 暗罩必须留在 base 层：clear_circle 是破坏性擦除，垫底才不会清掉波纹/拖尾。

name = "spotlight"
effect_name = "spotlight"
kind = "cursor"
description = "CursorFX spotlight：整屏压暗，指针处留柔和光圈。"
hide_cursor = true

function schema()
  return {
    { key = "radius",   label = "光圈半径", type = "int",   min = 40, max = 400, step = 1,    default = 170 },
    { key = "darkness", label = "暗罩浓度", type = "float", min = 0,  max = 1,   step = 0.01, default = 0.82 },
    { key = "softness", label = "边缘柔度", type = "float", min = 0,  max = 0.95, step = 0.01, default = 0.5 },
    { key = "color",    label = "暗罩颜色", type = "color", default = "#000000" },
  }
end

local px, py = 0, 0
local radius = 170
local vis = 0
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  px, py = 0, 0
  radius = 170
  vis = 0
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    px, py = mx, my
    radius = params.radius or 170
    started = true
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k = math.min(1.0, 0.35 * f)
  px = px + (mx - px) * k
  py = py + (my - py) * k

  local target = (params.radius or 170) * (mouse_down and 1.6 or 1.0) + (mouse_speed or 0) * 2.0
  radius = radius + (target - radius) * math.min(1.0, 0.15 * f)
  vis = vis + (1.0 - vis) * math.min(1.0, 0.08 * f)
end

function draw(ctx, params)
  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  -- ★ 必须在所有早退守卫之前：darkness=0（滑条最小值）时暗罩 alpha 为 0、首帧
  --   vis 未起量，旧实现整帧 return，唯一标出指针位置的点也不画了——箭头又被
  --   hide_cursor 藏了，用户完全找不到鼠标。放 top 层：render 时 top 永远最后画，
  --   不会被本帧 base 层的暗罩/clear_circle 破坏性擦除盖掉。
  ctx:layer("top")
  ctx:circle(mouse_x or px, mouse_y or py, 6.2, 0, 0, 0, 230)
  ctx:circle(mouse_x or px, mouse_y or py, 3.4, 255, 255, 255, 255)

  if not started or vis < 0.01 then return end

  local darkness = clampf(params.darkness or 0.82, 0.0, 1.0)
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#000000")
  local a = math.floor(255.0 * darkness * vis * ca / 255)
  if a <= 0 then return end
  local r = math.max(radius, 2.0)
  local soft = clampf(params.softness or 0.5, 0.0, 0.95)

  ctx:layer("base")
  ctx:rect(0, 0, screen_w or 0, screen_h or 0, cr, cg, cb, a)

  -- 先把整个光孔清透，再用径向渐变把“内圈透明 → 外沿恢复暗罩”的柔边补回来。
  -- 若只清内圈再叠渐变，渐变会和残留的暗罩二次叠加，外沿出现一圈更黑的接缝。
  ctx:clear_circle(px, py, r)
  if soft > 0.02 then
    ctx:radial_stops(px, py, r, {
      { offset = 0.0,  r = cr, g = cg, b = cb, a = 0 },
      { offset = soft, r = cr, g = cg, b = cb, a = 0 },
      { offset = 1.0,  r = cr, g = cg, b = cb, a = a },
    })
  end
end
