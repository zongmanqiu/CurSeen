-- 点击效果 · 水滴坠落
--
-- 两段叙事，同目录里唯一一个：先是一颗水滴"啪"地从上方砸到按下点，然后才溅起几滴水珠
-- 并荡开两圈涟漪。其它脚本都是"按下即炸开"，这一支有先后。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "水滴坠落"
name = "水滴坠落"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local drops = {}          -- 溅出的水珠 {x, y, vx, vy, life}
local splashed = false
local fall_h = 0.0        -- 本次下落高度（按下时定好，之后不变）
local SPLASH_G = 900.0    -- 溅珠重力（px/s²），固定值

function schema()
  return {
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#6EC6FF" },
    { key = "size",    label = "水滴大小", type = "float", min = 2,   max = 16,  step = 0.2, default = 5 },
    { key = "fall_ms", label = "下落时长", type = "int",   min = 100, max = 800, step = 10,  default = 260 },
    { key = "fall",    label = "下落高度", type = "int",   min = 0,   max = 900, step = 10,  default = 0 },
    { key = "splash",  label = "溅珠数",   type = "int",   min = 1,   max = 16,  step = 1,   default = 6 },
    { key = "rings",   label = "涟漪圈数", type = "int",   min = 1,   max = 4,   step = 1,   default = 2 },
    { key = "rest_ms", label = "余韵时长", type = "int",   min = 200, max = 1800, step = 10, default = 620 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
  drops = {}
  splashed = false

  -- 「下落高度 0」= 自动：从屏幕上方 24px 处砸下来，但最多砸 35% 屏高，
  -- 免得在屏幕下半部点击时水滴从很远的地方拖一条长尾，像流星而不是水滴。
  local h = p.fall or 0
  if h <= 0 then
    h = cy + 24.0
    local cap = screen_h * 0.35
    if h > cap then h = cap end
    if h < 24.0 then h = 24.0 end
  end
  fall_h = h
end

function step(dt, t, p)
  local ds = dt
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0.0 then return end
  age_ms = age_ms + dt * 1000.0

  local fms = p.fall_ms or 260
  if not splashed and age_ms >= fms then
    splashed = true
    local n = math.floor(p.splash or 6)
    if n < 1 then n = 1 end
    for i = 1, n do
      local ang = -1.5707963 + rng(-1.0, 1.0)     -- 向上为主的扇形
      local v = rng(90.0, 210.0)
      drops[i] = {
        x = cx, y = cy,
        vx = math.cos(ang) * v,
        vy = math.sin(ang) * v,
        life = rng(0.35, 0.6),
      }
    end
  end

  if splashed then
    for i = 1, #drops do
      local q = drops[i]
      q.vy = q.vy + SPLASH_G * ds
      q.x = q.x + q.vx * ds
      q.y = q.y + q.vy * ds
      q.life = q.life - ds
    end
  end
end

function draw(ctx, p)
  local fms = p.fall_ms or 260
  local rest = p.rest_ms or 620
  if age_ms >= fms + rest then return end     -- 两段都走完：停笔，实例随即被剔除

  local r, g, b, a = hex_to_rgb(p.color or "#6EC6FF")
  local size = p.size or 5.0

  if age_ms < fms then
    -- ── 段一：水滴下落（u² 加速，尾迹长度跟着瞬时速度） ──
    local u = age_ms / fms
    local y = cy - fall_h * (1.0 - u * u)
    local spd = 2.0 * fall_h * u / (fms * 0.001)
    local tail = spd * 0.03
    local cap = size * 3.2
    if tail > cap then tail = cap end
    local a8 = math.floor(a)
    if a8 > 0 then
      if tail > 1.5 then
        ctx:line(cx, y - tail, cx, y - 1.0, size * 0.55, r, g, b, math.floor(a * 0.5))
      end
      ctx:circle(cx, y, size * (0.8 + 0.25 * u), r, g, b, a8)
    end
  else
    -- ── 段二：溅珠 + 涟漪 ──
    local u2 = (age_ms - fms) / rest
    if u2 >= 1.0 then return end
    local fade = 1.0 - u2

    local rings = math.floor(p.rings or 2)
    if rings < 1 then rings = 1 end
    for i = 1, rings do
      local uu = u2 - ((i - 1) / rings) * 0.3      -- 后几圈延后起跑
      if uu > 0.0 and uu < 1.0 then
        local rr = 6.0 + 54.0 * uu
        local a8 = math.floor(a * fade * (1.0 - uu) * 0.9)
        if a8 > 0 then
          ctx:ring(cx, cy, rr, 1.8, r, g, b, a8)
        end
      end
    end

    for i = 1, #drops do
      local q = drops[i]
      if q.life > 0.0 then
        local la = q.life / 0.3
        if la > 1.0 then la = 1.0 end
        local a8 = math.floor(a * la * fade)
        if a8 > 0 then
          ctx:circle(q.x, q.y, size * 0.45, r, g, b, a8)
        end
      end
    end
  end
end
