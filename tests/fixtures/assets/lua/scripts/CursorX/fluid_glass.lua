-- CursorX · Fluid Glass
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：一枚真的会把桌面内容放大的玻璃透镜（`ctx:backdrop_circle` 抓取指针
-- 下方的屏幕，按 zoom 放大后以圆形蒙版画回），镜片带凸面高光、镜圈与投影，跟随有
-- 一点滞后；悬停时变大，点击胀开一次再弹回。
--
-- ★ 放大期间 CurSeen 覆盖层会被系统排除出截图/录屏（见 API.md 的 backdrop_circle），
--   这是为了不让镜片把自己上一帧抓进源图（无限套娃）。抓屏不可用时退回静态玻璃球。

name = "Fluid Glass"
effect_name = "Fluid Glass"
kind = "cursor"
description = "CursorX Fluid Glass：真正放大桌面内容的玻璃透镜。"
hide_cursor = true

local lx, ly = 0, 0
local hover_scale = 1.0
local size_scale = 1.0
local click_t = -1
local warned = false

function schema()
  return {
    { key = "lensSize",       label = "镜片大小", type = "int",   min = 24, max = 160, step = 2,    default = 50 },
    { key = "zoomLevel",      label = "放大倍率", type = "float", min = 1.0,max = 4.0, step = 0.1,  default = 1.2 },
    { key = "lerpSpeed",      label = "跟随速度", type = "float", min = 0.03,max = 0.5,step = 0.01, default = 0.1 },
    { key = "glassOpacity",   label = "玻璃高光", type = "float", min = 0, max = 0.8,   step = 0.05, default = 0.3 },
    { key = "rimStrength",    label = "镜圈亮度", type = "float", min = 0, max = 1.0,   step = 0.05, default = 0.0 },
    { key = "pointerAnim",    label = "悬停变大", type = "bool",  default = true },
    { key = "pointerSizeBoost", label = "悬停倍数", type = "float", min = 1.0, max = 2.0, step = 0.05, default = 1.25 },
    { key = "clickAnim",      label = "点击胀开", type = "bool",  default = true },
    { key = "clickExpand",    label = "胀开倍数", type = "float", min = 1.2, max = 3.0, step = 0.05, default = 1.5 },
  }
end

function init()
  lx, ly = mouse_x or 0, mouse_y or 0
  hover_scale = 1.0
  size_scale = 1.0
  click_t = -1
  warned = false
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or lx, mouse_y or ly
  local k = 1.0 - math.pow(1.0 - (params.lerpSpeed or 0.1), f)
  lx = lx + (mx - lx) * k
  ly = ly + (my - ly) * k

  local hover = cursor_hover and params.pointerAnim ~= false
  local target_hover = hover and (params.pointerSizeBoost or 1.25) or 1.0
  hover_scale = hover_scale + (target_hover - hover_scale) * math.min(1.0, 0.12 * f)

  if click_t >= 0 and params.clickAnim ~= false then
    size_scale = 1 + math.sin(click_t * math.pi) * ((params.clickExpand or 1.5) - 1)
    click_t = click_t + dt * 2.4
    if click_t >= 1 then size_scale = 1.0; click_t = -1 end
  else
    size_scale = 1.0
  end
end

function draw(ctx, params)
  params = params or {}
  local size = (params.lensSize or 50) * hover_scale * size_scale
  local radius = math.max(2.0, size * 0.5)
  local zoom = (params.zoomLevel or 1.2) / math.max(0.01, size_scale)

  ctx:layer("top")
  -- 投影：镜片外一圈暗边，让它在浅色桌面上也有轮廓。
  ctx:ring(lx, ly, radius + 2.0, 4.0, 0, 0, 0, 100)

  local ok = ctx:backdrop_circle(lx, ly, radius, "magnify", zoom, 255, 255, 255, 255)
  if not ok then
    -- 抓屏不可用：静态玻璃球（不放大，但保留高光/镜圈，至少能当光标用）。
    ctx:circle(lx, ly, radius, 200, 220, 245, 42)
    if not warned then
      warned = true
      if ext and ext.log then ext:log("Fluid Glass: 桌面采样不可用，已退回静态玻璃球") end
    end
  end

  local glass = params.glassOpacity or 0.3
  if glass > 0.01 then
    ctx:radial_stops(lx - radius * 0.30, ly - radius * 0.36, radius * 1.05, {
      { offset = 0.0,  r = 255, g = 255, b = 255, a = math.floor(255 * glass) },
      { offset = 0.36, r = 255, g = 255, b = 255, a = math.floor(255 * glass * 0.28) },
      { offset = 0.60, r = 255, g = 255, b = 255, a = 4 },
      { offset = 0.72, r = 255, g = 255, b = 255, a = 0 },
    })
  end

  local rim = params.rimStrength or 0.0
  if rim > 0.01 then
    ctx:ring(lx, ly, radius - 1.0, 1.6, 255, 255, 255, math.floor(255 * rim))
    ctx:ring(lx, ly, radius, 1.0, 0, 0, 0, math.floor(90 * rim))
  end

  -- 真实指针位置：镜片有滞后，给一个很小的高光点。
  local mx, my = mouse_x or lx, mouse_y or ly
  ctx:circle(mx, my, 1.8, 255, 255, 255, 150)
  ctx:circle(mx, my, 0.9, 0, 0, 0, 110)
end
