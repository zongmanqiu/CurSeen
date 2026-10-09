-- 点击效果 · 黑洞内旋
--
-- 一圈光点从四周**向内**螺旋收拢，越靠近中心转得越快，末了被中心细环一并吞掉。
-- 同目录其它脚本全是"向外 / 向上"的，这一支是唯一朝里的方向语汇。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "黑洞内旋"
name = "黑洞内旋"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local pts = {}            -- {a0, rr, k}

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#8AB4FF" },
    { key = "count",    label = "光点数",   type = "int",   min = 6,   max = 60,   step = 1,   default = 30 },
    { key = "radius",   label = "起始半径", type = "int",   min = 20,  max = 500,  step = 1,   default = 150 },
    { key = "duration", label = "吸入时长", type = "int",   min = 300, max = 3000, step = 10,  default = 900 },
    { key = "spin",     label = "旋转圈数", type = "float", min = 0.2, max = 4,    step = 0.05, default = 1.2 },
    { key = "size",     label = "光点大小", type = "float", min = 0.8, max = 8,    step = 0.1, default = 2.2 },
    { key = "core",     label = "中心细环", type = "bool",  default = true },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
  pts = {}

  local n = math.floor(p.count or 30)
  if n < 1 then n = 1 end
  local rad = p.radius or 150
  for i = 1, n do
    pts[i] = {
      a0 = (i - 1) * 6.2831853 / n + rng(-0.35, 0.35),   -- 起始角（均匀铺开再抖）
      rr = rad * rng(0.55, 1.0),                          -- 起始半径（层次感）
      k = rng(0.75, 1.3),                                 -- 收缩速率个体差
    }
  end
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 900
  local prog = age_ms / dur
  if prog >= 1.0 then return end       -- 全被吞掉：停笔，实例随即被剔除

  local r, g, b, a = hex_to_rgb(p.color or "#8AB4FF")
  local fade = 1.0 - prog
  local sz = p.size or 2.2
  local spin = (p.spin or 1.2) * 6.2831853

  for i = 1, #pts do
    local q = pts[i]
    -- 半径：prog^1.6 做内缩量（后段加速），越往后越"被拉进去"
    local e = prog * prog * math.sqrt(prog)
    local rr = q.rr * (1.0 - e)
    -- 角度：prog² 做转角（起点慢、末段飞快），切向速度 ∝ 1/r 的观感
    local ang = q.a0 + spin * prog * prog * q.k
    local a8 = math.floor(a * fade)
    if a8 > 0 then
      ctx:circle(cx + math.cos(ang) * rr, cy + math.sin(ang) * rr,
                 sz * (0.5 + 0.5 * fade), r, g, b, a8)
    end
  end

  if p.core ~= false then
    local a8 = math.floor(a * fade * 0.85)
    if a8 > 0 then
      -- ring 的第 3 参是**外半径**、第 4 参是从外径向内收的笔宽
      ctx:ring(cx, cy, 5.0 + 9.0 * prog, 1.6, r, g, b, a8)
    end
  end
end
