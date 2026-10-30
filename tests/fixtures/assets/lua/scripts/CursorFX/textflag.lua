-- CursorFX · textflag / 复刻自 cursorfx-main/src/cursors/textflag.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：一串文字像旗子一样跟在指针后面，字符之间保持最小间距；每个字沿绳子的
-- 法线做正弦摆动，越靠后摆幅越大。头部是一颗白点。
--
-- 实现：CurSeen 没有文字旋转，字符一律正立，只做位置上的正弦摆动；原版 3px 黑色
-- 描边改成偏移 1px 的深色投影（两次 text_centered）。UTF-8 按字节长度手工拆字，
-- 中文也能逐字排列；最多 20 字，避免一帧几十次文字光栅化。
--
-- 性能：默认 9 字 × 2 次文字 = 18 次文字绘制 + 1 个白点，文字是该效果的主要成本。

name = "textflag"
effect_name = "textflag"
kind = "cursor"
description = "CursorFX textflag：像旗子一样飘动的文字光标。"
hide_cursor = true

local MAX_CHARS = 20

function schema()
  return {
    { key = "text",  label = "文字",   type = "str",   default = "CURSORFX" },
    { key = "color", label = "颜色",   type = "color", alpha = true, default = "#FFD23F" },
    { key = "gap",   label = "字距",   type = "float", min = 8,  max = 40, step = 1, default = 12 },
    { key = "size",  label = "字号",   type = "float", min = 8,  max = 40, step = 1, default = 16 },
  }
end

local xs, ys = {}, {}
local count = 0
local chars = {}
local text_cache = ""

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

-- 极简 UTF-8 拆分：按首字节判断序列长度，非法字节按单字节处理。
local function split_utf8(s)
  local out = {}
  local i = 1
  while i <= #s and #out < MAX_CHARS do
    local b = s:byte(i)
    local n = 1
    if b >= 0xF0 then
      n = 4
    elseif b >= 0xE0 then
      n = 3
    elseif b >= 0xC0 then
      n = 2
    end
    out[#out + 1] = s:sub(i, i + n - 1)
    i = i + n
  end
  return out
end

function init()
  xs, ys = {}, {}
  count = 0
  chars = {}
  text_cache = ""
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  local raw = params.text
  if raw == nil or raw == "" then raw = "CURSORFX" end

  if raw ~= text_cache then
    text_cache = raw
    chars = split_utf8(raw)
    if #chars == 0 then chars = { "?" } end
    count = #chars + 1
    xs, ys = {}, {}
    for i = 1, count do
      xs[i], ys[i] = mx, my
    end
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local k0 = math.min(1.0, 0.7 * f)
  local k = math.min(1.0, 0.4 * f)
  local gap = math.max(1.0, params.gap or 12)

  xs[1] = xs[1] + (mx - xs[1]) * k0
  ys[1] = ys[1] + (my - ys[1]) * k0
  for i = 2, count do
    xs[i] = xs[i] + (xs[i - 1] - xs[i]) * k
    ys[i] = ys[i] + (ys[i - 1] - ys[i]) * k

    local dx, dy = xs[i] - xs[i - 1], ys[i] - ys[i - 1]
    local d = math.sqrt(dx * dx + dy * dy)
    if d < gap then
      if d < 0.001 then
        dx, dy, d = 1.0, 0.0, 1.0
      end
      xs[i] = xs[i - 1] + dx / d * gap
      ys[i] = ys[i - 1] + dy / d * gap
    end
  end
end

function draw(ctx, params)
  if count < 2 then return end

  local time = elapsed or 0
  local size = params.size or 16
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFD23F")

  for i = 2, count do
    local ch = chars[i - 1]
    if ch then
      local dx = xs[i - 1] - xs[i]
      local dy = ys[i - 1] - ys[i]
      local a = math.atan(dy, dx)
      local wave = math.sin(time * 8.0 - i * 0.6) * (2.0 + i * 0.4)
      local x = xs[i] - math.sin(a) * wave
      local y = ys[i] + math.cos(a) * wave

      ctx:text_centered(x + 1.0, y + 1.0, ch, size, 0, 0, 0, 130)
      ctx:text_centered(x, y, ch, size, cr, cg, cb, ca)
    end
  end

  ctx:circle(mouse_x or xs[1], mouse_y or ys[1], 3.0, 255, 255, 255, 255)
end
