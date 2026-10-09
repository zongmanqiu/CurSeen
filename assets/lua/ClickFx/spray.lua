-- 点击效果 · 彩色爆散
--
-- 一簇彩色粒子从按下点向四周炸开：受空气阻力减速、可带重力下坠，边飞边淡。
-- 与同目录的「实心圆 / 多重光环」是两套语汇——那两支是连续的"面"，这一支是离散的"点"。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "彩色爆散"
name = "彩色爆散"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0
local parts = {}          -- {x, y, vx, vy, r, g, b, rd, al}
local DRAG = 2.6          -- 空气阻力（1/秒）。固定值：面板参数越少越清爽

function schema()
  return {
    { key = "rainbow", label = "彩虹色",   type = "bool",  default = true },
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#FFD24A" },
    { key = "count",   label = "粒子数",   type = "int",   min = 6,   max = 60,   step = 1,   default = 26 },
    { key = "speed",   label = "初速",     type = "float", min = 40,  max = 1200, step = 10,  default = 340 },
    { key = "gravity", label = "重力",     type = "float", min = 0,   max = 2000, step = 20,  default = 220 },
    { key = "size",    label = "粒子大小", type = "float", min = 1,   max = 12,   step = 0.2, default = 3.2 },
    { key = "life",    label = "寿命",     type = "int",   min = 150, max = 3000, step = 10,  default = 700 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
  parts = {}

  local n = math.floor(p.count or 26)
  if n < 1 then n = 1 end
  local spd = p.speed or 340
  local size = p.size or 3.2
  local pr, pg, pb, pa = hex_to_rgb(p.color or "#FFD24A")
  local rainbow = p.rainbow
  if rainbow == nil then rainbow = true end

  local al = pa / 255.0
  for i = 1, n do
    local ang = rng(0.0, 6.2831853)
    local v = spd * rng(0.35, 1.0)
    local r, g, b
    if rainbow then
      -- 色轮等分再加抖动；★ 取模保证 h 落在 0..360（Lua 的 % 是向下取整，负数也回正），
      -- 免得 hsv_to_rgb 收到负色相时和宿主（Rust 的 % 是截断）算出不同颜色。
      local h = ((i - 1) * 360.0 / n + rng(-14.0, 14.0)) % 360.0
      r, g, b = hsv_to_rgb(h, 0.85, 1.0)
    else
      local k = rng(0.65, 1.0)
      r, g, b = math.floor(pr * k), math.floor(pg * k), math.floor(pb * k)
    end
    parts[i] = {
      x = x, y = y,
      vx = math.cos(ang) * v,
      vy = math.sin(ang) * v,
      r = r, g = g, b = b,
      rd = size * rng(0.6, 1.25),
      al = al,
    }
  end
end

function step(dt, t, p)
  if #parts == 0 then return end
  -- 单帧最多按 64ms 推进：卡顿帧不把粒子瞬移出屏（寿命仍按真实 dt 走）。
  local ds = dt
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0.0 then return end
  age_ms = age_ms + dt * 1000.0

  local g = p.gravity or 220
  local k = 1.0 - DRAG * ds
  if k < 0.0 then k = 0.0 end
  for i = 1, #parts do
    local q = parts[i]
    q.vx = q.vx * k
    q.vy = q.vy * k + g * ds
    q.x = q.x + q.vx * ds
    q.y = q.y + q.vy * ds
  end
end

function draw(ctx, p)
  local life = p.life or 700
  local prog = age_ms / life
  if prog >= 1.0 then return end       -- 全部散尽：停笔，实例随即被剔除

  local fade = 1.0 - prog
  local dim = 0.35 + 0.65 * fade       -- 同时收缩一点，像被空气吃掉
  for i = 1, #parts do
    local q = parts[i]
    local a8 = math.floor(q.al * fade * 255.0 + 0.5)
    if a8 > 0 then
      ctx:circle(q.x, q.y, q.rd * dim, q.r, q.g, q.b, a8)
    end
  end
end
