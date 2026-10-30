-- CursorFX · pixel / 复刻自 cursorfx-main/src/cursors/pixel.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：粗颗粒 8-bit 箭头，左上角对准指针；按下时横向变宽、纵向压扁。
--
-- 实现：原始 18×12 网格逐格重画会有 100 多个图元，这里把每行连续同色的格子合并成
-- 一个矩形（约 55~65 个 rect），观感完全一致、指令量减半。变换直接算到矩形坐标：
-- x 乘 1/squash、y 乘 squash，等价于原版的 canvas scale。
--
-- 性能：极限约 65 个轴对齐矩形，无文字/渐变，单帧成本很低。

name = "pixel"
effect_name = "pixel"
kind = "cursor"
description = "CursorFX pixel：会压扁的 8-bit 像素箭头。"
hide_cursor = true

local MAP = {
  "X...........",
  "XX..........",
  "XoX.........",
  "XooX........",
  "XoooX.......",
  "XooooX......",
  "XoooooX.....",
  "XooooooX....",
  "XoooooooX...",
  "XooooooooX..",
  "XoooooooooX.",
  "XoooooXXXXXX",
  "XooXooX.....",
  "XoX.XooX....",
  "XX..XooX....",
  "X....XooX...",
  ".....XooX...",
  "......XX....",
}

function schema()
  return {
    { key = "fill",    label = "填充色", type = "color", default = "#FFFFFF" },
    { key = "outline", label = "描边色", type = "color", default = "#000000" },
    { key = "scale",   label = "像素大小", type = "float", min = 1, max = 8, step = 0.5, default = 3 },
  }
end

local squash = 1.0

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

function init()
  squash = 1.0
end

function step(dt, t, params)
  local f = clampf(dt * 60.0, 0.0, 2.0)
  local target = mouse_down and 0.8 or 1.0
  squash = squash + (target - squash) * math.min(1.0, 0.3 * f)
end

function draw(ctx, params)
  local s = params.scale or 3.0
  local mx, my = mouse_x or 0, mouse_y or 0
  local fr, fg, fb, fa = hex_to_rgb(params.fill or "#FFFFFF")
  local orr, og, ob, oa = hex_to_rgb(params.outline or "#000000")
  local sx = 1.0 / squash
  local sy = squash

  for row = 1, #MAP do
    local line = MAP[row]
    local col = 1
    while col <= #line do
      local c = line:sub(col, col)
      if c == "." then
        col = col + 1
      else
        local start_col = col
        repeat
          col = col + 1
        until col > #line or line:sub(col, col) ~= c

        local n = col - start_col
        local cr, cg, cb, ca
        if c == "X" then
          cr, cg, cb, ca = orr, og, ob, oa
        else
          cr, cg, cb, ca = fr, fg, fb, fa
        end
        ctx:rect(
          mx + (start_col - 1) * s * sx,
          my + (row - 1) * s * sy,
          n * s * sx,
          s * sy,
          cr, cg, cb, ca
        )
      end
    end
  end
end
