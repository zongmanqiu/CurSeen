-- 点击效果 · 点击喷泉
--
-- 机制上与同目录其它脚本都不一样：别的都是"按下瞬间炸一次"，这一支是按下后**先立起一个
-- 有寿命的喷口**，按速率持续往外喷粒子，喷完收工、粒子落尽即止——"单击 → 持续一会儿"。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）；发射与物理都放这里
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "点击喷泉"
name = "点击喷泉"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local parts = {}          -- {x, y, vx, vy, r, g, b, al, life, sz}
local acc = 0.0           -- 发射累加器（不足 1 粒就攒着）
local G = 620.0           -- 粒子重力（px/s²），固定值
local P_SIZE = 3.0        -- 粒子基准半径，固定值

function schema()
  return {
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#FFD24A" },
    { key = "rainbow", label = "彩虹色",   type = "bool",  default = false },
    { key = "rate",    label = "喷射速率", type = "int",   min = 2,   max = 60,   step = 1,   default = 18 },
    { key = "emit_ms", label = "喷射时长", type = "int",   min = 100, max = 2000, step = 10,  default = 420 },
    { key = "speed",   label = "喷射初速", type = "float", min = 80,  max = 900,  step = 10,  default = 340 },
    { key = "spread",  label = "散布角",   type = "float", min = 5,   max = 90,   step = 1,   default = 34 },
    { key = "life",    label = "粒子寿命", type = "int",   min = 200, max = 2000, step = 10,  default = 700 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms, acc = x, y, 0.0, 0.0
  parts = {}
end

function step(dt, t, p)
  local ds = dt
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0.0 then return end
  age_ms = age_ms + dt * 1000.0

  -- ── 发射：不是"每帧一粒"，而是按速率攒够一粒就发一粒 ──
  if age_ms <= (p.emit_ms or 420) then
    acc = acc + (p.rate or 18) * ds
    local spd = p.speed or 340
    local spread = (p.spread or 34) * 0.017453292
    local pr, pg, pb, pa = hex_to_rgb(p.color or "#FFD24A")
    local rainbow = p.rainbow
    local life_s = (p.life or 700) * 0.001
    -- 上限 64ms 的步长 × 60/s 的速率 → 单帧最多补 4 粒，不可能长循环
    while acc >= 1.0 do
      acc = acc - 1.0
      local ang = -1.5707963 + rng(-spread, spread)     -- 以正上方为轴的喇叭口
      local v = spd * rng(0.75, 1.15)
      local r, g, b
      if rainbow then
        r, g, b = hsv_to_rgb(rng(0.0, 360.0) % 360.0, 0.8, 1.0)
      else
        r, g, b = pr, pg, pb
      end
      parts[#parts + 1] = {
        x = cx, y = cy,
        vx = math.cos(ang) * v,
        vy = math.sin(ang) * v,
        r = r, g = g, b = b,
        al = pa / 255.0,
        life = life_s,
        sz = P_SIZE * rng(0.7, 1.2),
      }
    end
  end

  -- ── 推进 + 剔除（粒子寿命尽就地回收，不留死数据） ──
  local keep = {}
  for i = 1, #parts do
    local q = parts[i]
    q.vy = q.vy + G * ds
    q.x = q.x + q.vx * ds
    q.y = q.y + q.vy * ds
    q.life = q.life - ds
    if q.life > 0.0 then
      keep[#keep + 1] = q
    end
  end
  parts = keep
end

function draw(ctx, p)
  local emit_ms = p.emit_ms or 420
  local life_s = (p.life or 700) * 0.001
  local r, g, b, a = hex_to_rgb(p.color or "#FFD24A")

  -- ★ 喷射期的喷口暖光。不只是好看：发射器刚出生、累加器还没攒够第一粒时粒子表是空的，
  --   若此刻零指令，实例会在第一帧就被判死（"某帧不画即消亡"）——喷口保证它活着。
  if age_ms <= emit_ms then
    ctx:radial(cx, cy, P_SIZE * 2.8, r, g, b, math.floor(a * 0.6))
  end

  if #parts == 0 then return end        -- 粒子落尽且已停喷：停笔，实例随即被剔除

  for i = 1, #parts do
    local q = parts[i]
    local u = 1.0 - q.life / life_s
    if u < 0.0 then u = 0.0 end
    local a8 = math.floor(q.al * (1.0 - u) * 255.0 + 0.5)
    if a8 > 0 then
      ctx:circle(q.x, q.y, q.sz * (0.5 + 0.6 * (1.0 - u)), q.r, q.g, q.b, a8)
    end
  end
end
