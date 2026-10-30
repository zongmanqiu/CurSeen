-- 点击效果 · 星形扩散
--
-- 一枚矢量多角星从按下点弹出：边放大边自旋，可带重力下坠，末段淡出。
-- 同目录其它脚本都是"圆/环/点"，这一支是唯一具象图形。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "星形扩散"
name = "星形扩散"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#FFD24A" },
    { key = "max_size", label = "最大半径", type = "int",   min = 10,  max = 300,  step = 1,   default = 44 },
    { key = "duration", label = "扩散时长", type = "int",   min = 150, max = 2000, step = 10,  default = 520 },
    { key = "points",   label = "角数",     type = "int",   min = 3,   max = 12,   step = 1,   default = 5 },
    { key = "inner",    label = "内外比",   type = "float", min = 0.2, max = 0.9,  step = 0.02, default = 0.42 },
    { key = "spin",     label = "自旋圈数", type = "float", min = -2,  max = 2,    step = 0.05, default = 0.3 },
    { key = "fall",     label = "下坠",     type = "float", min = 0,   max = 1500, step = 20,  default = 0 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 520
  local prog = age_ms / dur
  if prog >= 1.0 then return end       -- 淡尽：停笔，实例随即被剔除

  local r, g, b, a = hex_to_rgb(p.color or "#FFD24A")
  local a8 = math.floor(a * (1.0 - prog))
  if a8 <= 0 then return end

  local n = math.floor(p.points or 5)
  if n < 3 then n = 3 end
  local inner = p.inner or 0.42
  local ease = 1.0 - (1.0 - prog) * (1.0 - prog) * (1.0 - prog)   -- easeOutCubic
  local rad = (p.max_size or 44) * (0.15 + 0.85 * ease)

  -- 重力下坠：位移 = ½·g·t²（t 用秒）
  local ts = age_ms * 0.001
  local dy = 0.5 * (p.fall or 0) * ts * ts

  local spin = (p.spin or 0.3) * 6.2831853 * prog
  local step_a = 3.14159265 / n          -- 相邻顶点半角
  local pts = {}
  for i = 0, 2 * n - 1 do
    local ang = -1.5707963 + spin + i * step_a
    local rr = rad
    if i % 2 == 1 then rr = rad * inner end
    pts[#pts + 1] = { cx + math.cos(ang) * rr, cy + dy + math.sin(ang) * rr }
  end
  ctx:polygon(pts, r, g, b, a8)
end
