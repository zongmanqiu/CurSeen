-- 点击效果 · 神经结网
--
-- 一簇节点从按下点散开，彼此靠得够近就自动连成线——连线随距离变淡、随整体淡出断开，
-- 最后整张网一起消失。全项目唯一"元素之间自动连线"的结构感形态。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "神经结网"
name = "神经结网"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local nodes = {}          -- {dx, dy, k, ph}

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#7DF9FF" },
    { key = "nodes",    label = "节点数",   type = "int",   min = 4,   max = 24,   step = 1,   default = 10 },
    { key = "reach",    label = "散开半径", type = "int",   min = 30,  max = 400,  step = 1,   default = 130 },
    { key = "range",    label = "连线距离", type = "int",   min = 30,  max = 300,  step = 1,   default = 110 },
    { key = "life",     label = "存活时长", type = "int",   min = 300, max = 2500, step = 10,  default = 900 },
    { key = "size",     label = "节点大小", type = "float", min = 1,   max = 6,    step = 0.1, default = 2.0 },
    { key = "link_w",   label = "连线粗细", type = "float", min = 0.5, max = 4,    step = 0.1, default = 1.0 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
  nodes = {}

  local n = math.floor(p.nodes or 10)
  if n < 2 then n = 2 end
  for i = 1, n do
    local ang = (i - 1) * 6.2831853 / n + rng(-0.22, 0.22)
    nodes[i] = {
      dx = math.cos(ang),
      dy = math.sin(ang),
      k = rng(0.65, 1.15),              -- 个体散开距离系数
    }
  end
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local life = p.life or 900
  local prog = age_ms / life
  if prog >= 1.0 then return end       -- 淡尽：停笔，实例随即被剔除

  local r, g, b, a = hex_to_rgb(p.color or "#7DF9FF")
  local fade = 1.0 - prog
  local n = #nodes
  local reach = p.reach or 130
  local range = p.range or 110

  -- 减速外扩：位置先快后慢（easeOutQuad）
  local ease = 1.0 - (1.0 - prog) * (1.0 - prog)
  local px, py = {}, {}
  for i = 1, n do
    local q = nodes[i]
    local d = reach * q.k * ease
    px[i] = cx + q.dx * d
    py[i] = cy + q.dy * d
  end

  -- 连线：两两比距离，近了才连（n ≤ 24 → 最多 276 次比较，Lua 侧毫无压力）
  local lw = p.link_w or 1.0
  local r2 = range * range
  for i = 1, n do
    for j = i + 1, n do
      local dx = px[j] - px[i]
      local dy = py[j] - py[i]
      local d2 = dx * dx + dy * dy
      if d2 > 0.0001 and d2 < r2 then
        local k = 1.0 - math.sqrt(d2) / range
        local a8 = math.floor(a * fade * k * 0.9)
        if a8 > 0 then
          ctx:line(px[i], py[i], px[j], py[j], lw * (0.4 + 0.6 * k), r, g, b, a8)
        end
      end
    end
  end

  -- 节点画在线上面（先线后点，点才清晰）
  local a8 = math.floor(a * fade)
  if a8 > 0 then
    local sz = p.size or 2.0
    for i = 1, n do
      ctx:circle(px[i], py[i], sz * (0.6 + 0.4 * fade), r, g, b, a8)
    end
  end
end
