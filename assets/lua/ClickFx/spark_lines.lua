-- 点击效果 · 星芒射线
--
-- 几根圆头线段从按下点向外甩出，各自错峰起跑、撑开到位后一起淡掉。
-- 全项目唯一"线"语汇（其它都是点/环/面），极干净。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "星芒射线"
name = "星芒射线"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local STAGGER = 14.0      -- 相邻射线的起跑间隔（毫秒）

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#FFFFFF" },
    { key = "count",    label = "射线数",   type = "int",   min = 3,   max = 16,   step = 1,   default = 8 },
    { key = "length",   label = "长度",     type = "int",   min = 10,  max = 300,  step = 1,   default = 46 },
    { key = "width",    label = "粗细",     type = "float", min = 1,   max = 8,    step = 0.2, default = 2.4 },
    { key = "gap",      label = "内空",     type = "float", min = 0,   max = 0.8,  step = 0.02, default = 0.18 },
    { key = "tilt",     label = "起始角",   type = "float", min = 0,   max = 90,   step = 1,   default = 20 },
    { key = "duration", label = "单根时长", type = "int",   min = 120, max = 1200, step = 10,  default = 380 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local n = math.floor(p.count or 8)
  if n < 1 then n = 1 end
  local dur = p.duration or 380
  local len = p.length or 46
  local w = p.width or 2.4
  local r0 = len * (p.gap or 0.18)
  local tilt = (p.tilt or 20) * 0.017453292
  local r, g, b, a = hex_to_rgb(p.color or "#FFFFFF")

  local step_a = 6.2831853 / n
  for i = 1, n do
    local t0 = (i - 1) * STAGGER
    local u = (age_ms - t0) / dur
    -- ★ 用 `>= 0` 而不是 `> 0`：按下事件可能落在两帧之间，实例会在 age 还是 0 时就被画一次，
    --   那一刻若一条线都不产出，首帧零指令会被判死（宿主"某帧不画即消亡"）。
    if u >= 0.0 and u < 1.0 then      -- 未起跑 / 已淡尽：都不画
      local e = 1.0 - (1.0 - u) * (1.0 - u) * (1.0 - u)   -- easeOutCubic：先冲后缓
      -- 至少留 0.5px：age=0 时 e=0，零长线段没有意义
      local r1 = r0 + math.max(0.5, (len - r0) * e)
      local fade = 1.0 - u
      local a8 = math.floor(a * fade)
      if a8 > 0 then
        local ang = tilt + (i - 1) * step_a
        local ca, sa = math.cos(ang), math.sin(ang)
        ctx:line(cx + ca * r0, cy + sa * r0, cx + ca * r1, cy + sa * r1,
                 w * (0.35 + 0.65 * fade), r, g, b, a8)
      end
    end
  end
end
