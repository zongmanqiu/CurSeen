-- 点击效果 · 实心圆
--
-- 一颗实心圆从按下点扩散并淡出，边缘干净，叠在浅色背景上是一团柔和色斑。
--
-- §39 起本目录收敛为两支（实心圆 / 多重光环）：颜色统一**自带透明度**
-- （`alpha = true` → `hex_to_rgb` 第四返回值，取色器的透明度条给），
-- 独立的「不透明度」参数已删除。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**；老化必须放这里，否则鼠标不动就不会老化）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除

effect_name = "实心圆"
name = "实心圆"
role = "effect"
function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#FF0000" },
    { key = "max_size", label = "最大半径", type = "int",   min = 20, max = 400, step = 1, default = 50 },
    { key = "duration", label = "扩散时长", type = "int",   min = 100, max = 3000, step = 10, default = 500 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age = x, y, 0
end

function step(dt, t, p)
  age = (age or 0) + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 500
  local prog = math.min((age or 0) / dur, 1.0)
  if prog >= 1.0 then return end          -- 淡出完毕：停止绘制，实例随即被剔除

  -- 最终不透明度 = 颜色自带 alpha × 淡出进度（1 → 0）。
  local r, g, b, a = hex_to_rgb(p.color or "#FF0000")
  local max_r = p.max_size or 50
  local radius = max_r * (0.2 + prog * 0.8)
  ctx:circle(cx, cy, radius, r, g, b, math.floor(a * (1.0 - prog)))
end
