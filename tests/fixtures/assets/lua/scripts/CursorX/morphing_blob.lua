-- CursorX · Morphing Blob
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：一颗软边主 blob 钉在指针上，后面串一串逐渐变小的跟随 blob，叠在一起
-- 像一团会流动的颜料；点击在六种预设色之间循环（选中的颜色会一直保持到下次点击）。
-- 简化：没有 SVG gooey 滤镜，用多层软边径向渐变模拟融合；悬停时不变成网页元素形状，
-- 改为收拢成一圈包住指针的软环。

name = "Morphing Blob"
effect_name = "Morphing Blob"
kind = "cursor"
description = "CursorX Morphing Blob：流动的软边颜料团，点击换色。"
hide_cursor = true

local COLORS = {
  { 124, 92, 252 }, { 92, 244, 252 }, { 252, 92, 184 },
  { 92, 252, 168 }, { 252, 168, 92 }, { 255, 68, 85 },
}

local blobs = {}
local color_idx = 0
local color_src = ""
local wrapper_alpha = 0.65
local cur_size = 60

function schema()
  return {
    { key = "color",      label = "Blob 颜色", type = "color", alpha = true, default = "#7C5CFC" },
    { key = "size",       label = "Blob 大小", type = "int",   min = 20, max = 90,  step = 2,    default = 60 },
    { key = "trail",      label = "拖尾数量", type = "int",   min = 2,  max = 12,  step = 1,    default = 4 },
    { key = "lerp",       label = "跟随速度", type = "float", min = 0.05,max = 0.8,step = 0.05, default = 0.4 },
    { key = "opacity",    label = "不透明度", type = "float", min = 0.2, max = 1.0, step = 0.05, default = 0.8 },
    { key = "pointerAnim",label = "悬停收拢", type = "bool",  default = true },
    { key = "clickAnim",  label = "点击换色", type = "bool",  default = true },
  }
end

function init()
  blobs = {}
  color_idx = 0
  color_src = ""
  wrapper_alpha = 0.65
  cur_size = 60
end

local function rebuild(params)
  local n = math.floor(params.trail or 4)
  local mx, my = mouse_x or 0, mouse_y or 0
  blobs = {}
  for i = 1, n + 1 do
    blobs[i] = { x = mx, y = my }
  end
end

local function current_color(params)
  if color_idx > 0 then
    local c = COLORS[color_idx]
    return c[1], c[2], c[3], 255
  end
  return hex_to_rgb(params.color or "#7C5CFC")
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  -- 0 = 参数里的颜色；必须能轮回到 0，否则第一次点击后参数颜色就永远失效。
  color_idx = (color_idx + 1) % (#COLORS + 1)
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  if #blobs ~= math.floor(params.trail or 4) + 1 then rebuild(params) end

  local mx, my = mouse_x or 0, mouse_y or 0
  blobs[1].x, blobs[1].y = mx, my

  local lerp = params.lerp or 0.4
  for i = 2, #blobs do
    local prev = blobs[i - 1]
    local k = 1.0 - math.pow(1.0 - lerp, f)
    blobs[i].x = blobs[i].x + (prev.x - blobs[i].x) * k
    blobs[i].y = blobs[i].y + (prev.y - blobs[i].y) * k
  end

  local hover = cursor_hover and params.pointerAnim ~= false
  local base_op = params.opacity or 0.8
  local target_op = hover and base_op * 0.45 or base_op
  wrapper_alpha = wrapper_alpha + (target_op - wrapper_alpha) * math.min(1.0, 0.1 * f)

  local target_size = params.size or 60
  if hover then target_size = target_size * 0.72 end
  cur_size = cur_size + (target_size - cur_size) * math.min(1.0, 0.12 * f)
end

function draw(ctx, params)
  params = params or {}
  -- 调色要立刻生效：重置到“参数颜色”，而不是继续在预设色里循环。
  if params.color ~= color_src then
    color_src = params.color
    color_idx = 0
  end
  local r, g, b, ca = current_color(params)
  local a = math.floor(255 * clamp(wrapper_alpha * ca / 255, 0, 1))
  local lead_r = math.max(4.0, cur_size * 0.5)
  local hover = cursor_hover and params.pointerAnim ~= false

  for i = 1, #blobs do
    local blob = blobs[i]
    local rad = i == 1 and lead_r or lead_r * 0.72
    local al = i == 1 and a or math.floor(a * 0.8)
    -- 软边：中心实、外沿透明，叠在一起像融合的颜料。
    ctx:radial_stops(blob.x, blob.y, rad, {
      { offset = 0.0,  r = r, g = g, b = b, a = al },
      { offset = 0.68, r = r, g = g, b = b, a = math.floor(al * 0.82) },
      { offset = 1.0,  r = r, g = g, b = b, a = 0 },
    })
  end

  if hover then
    ctx:ring(blobs[1].x, blobs[1].y, lead_r * 1.18, 2.0, r, g, b, math.floor(a * 0.55))
  end
  -- 指针精确定位点（软团中心可能不易判断）。
  ctx:circle(blobs[1].x, blobs[1].y, 1.6, 255, 255, 255, 200)
end
