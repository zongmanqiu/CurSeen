-- 点击效果 · 点阵波纹
--
-- 一片方阵光点，一圈环形波从按下点向外传过去，每点按波的相位胀大 / 变亮，整片一起淡出。
-- 全项目唯一"波在格子里传播"的形态（其它波纹都是画出来的环，这一支是点自己动）。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "点阵波纹"
name = "点阵波纹"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local WAVE_LEN = 2.4      -- 波长（单位：格）。固定值，让面板只留真正想调的

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#4ADE80" },
    { key = "grid",     label = "网格边长", type = "int",   min = 3,   max = 11,   step = 2,   default = 7 },
    { key = "spacing",  label = "点间距",   type = "int",   min = 8,   max = 80,   step = 1,   default = 26 },
    { key = "amp",      label = "起伏幅度", type = "float", min = 0.1, max = 1.0,  step = 0.02, default = 0.55 },
    { key = "speed",    label = "波速",     type = "float", min = 0.5, max = 8,    step = 0.1, default = 2.2 },
    { key = "duration", label = "持续时长", type = "int",   min = 300, max = 3000, step = 10,  default = 1100 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 1100
  local prog = age_ms / dur
  if prog >= 1.0 then return end       -- 淡尽：停笔，实例随即被剔除

  local n = math.floor(p.grid or 7)
  if n < 1 then n = 1 end
  local sp = p.spacing or 26
  local amp = p.amp or 0.55
  local r, g, b, a = hex_to_rgb(p.color or "#4ADE80")

  local fade = 1.0 - prog
  local ts = age_ms * 0.001
  local k = 6.2831853 / (sp * WAVE_LEN)          -- 空间角频率（rad / px）
  local w = 6.2831853 * (p.speed or 2.2)         -- 时间角频率（rad / s）
  local half = (n - 1) * 0.5
  local BASE = 2.0                               -- 基点半径

  for gy = 0, n - 1 do
    for gx = 0, n - 1 do
      local dx = (gx - half) * sp
      local dy = (gy - half) * sp
      local d = math.sqrt(dx * dx + dy * dy)
      local v = 0.5 + 0.5 * math.sin(d * k - ts * w)     -- 0..1
      local a8 = math.floor(a * fade * (0.3 + 0.7 * v))
      if a8 > 0 then
        local rr = BASE * (1.0 + amp * (2.0 * v - 1.0))
        if rr < 0.4 then rr = 0.4 end     -- 幅度拉满时谷底会算出 0，垫个下限
        ctx:circle(cx + dx, cy + dy, rr, r, g, b, a8)
      end
    end
  end
end
