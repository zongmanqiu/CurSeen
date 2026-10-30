-- 气泡升腾：沿轨迹冒出一个个描边小圆，向上加速浮起、边浮边长大、边左右飘，
-- 到寿就缩着淡出。
--
-- 向上加速浮起 + 轻微横向飘移 + 半径逐渐长大。
--
-- ★ 与 `pixie_dust`（像素尘埃）正好相反：它向下掉，这套向上浮；
--   而且气泡必须是"空心描边"才像气泡——CurSeen 的 `ring()` 直接给出描边圆。
--
-- ★ 空闲归零：气泡全部落空后停止绘制，静止 0 开销。

effect_name = "气泡升腾"
name = "气泡升腾"
role = "effect"
function schema()
  return {
    { key = "opacity",    label = "不透明度", type = "float", min = 0.05, max = 1,  step = 0.01, default = 0.9 },
    { key = "life",       label = "气泡寿命", type = "float", min = 0.2,  max = 4,  step = 0.1,  default = 1.4 },
    { key = "interval_ms", label = "冒出间隔", type = "int",  min = 4,    max = 200, step = 2,   default = 26 },
    { key = "spacing",    label = "最小间距", type = "float", min = 2,    max = 80, step = 1,    default = 14 },
    { key = "size",       label = "初始半径", type = "float", min = 1,    max = 40, step = 1,    default = 6 },
    { key = "grow",       label = "长大倍数", type = "float", min = 0.5,  max = 2.5,step = 0.05, default = 1.2 },
    { key = "rise",       label = "上浮速度", type = "float", min = 0,    max = 300,step = 5,    default = 70 },
    { key = "drift",      label = "左右飘",   type = "float", min = 0,    max = 60, step = 1,    default = 14 },
    { key = "stroke",     label = "描边粗细", type = "float", min = 0.5,  max = 8,  step = 0.5,  default = 2 },
    { key = "fill",       label = "填充色",   type = "color", default = "#E6F1F7" },
    { key = "line",       label = "描边色",   type = "color", default = "#3A92C5" },
  }
end

bubbles = {}
elapsed = 0.0
last_spawn_t = -1.0
last_spawn_x, last_spawn_y = 0.0, 0.0

function init()
  bubbles = {}
  elapsed = 0.0
  last_spawn_t = -1.0
  last_spawn_x, last_spawn_y = 0.0, 0.0
end

function on_move(x, y, t, params)
  elapsed = t
  local interval = (params.interval_ms or 26) / 1000.0
  local spacing = params.spacing or 14
  local dx, dy = x - last_spawn_x, y - last_spawn_y
  local far_enough = math.sqrt(dx * dx + dy * dy) >= spacing
  local time_enough = (t - last_spawn_t) >= interval
  if last_spawn_t >= 0.0 and not far_enough and not time_enough then
    return
  end
  last_spawn_t = t
  last_spawn_x, last_spawn_y = x, y

  local r0 = params.size or 6
  local life = params.life or 1.4
  bubbles[#bubbles + 1] = {
    x = x, y = y,
    -- 初速：向上的主分量交给 `rise` 在 step 里加速，这里只给一点横向随机
    vx = rng(-1.0, 1.0) * (params.drift or 14) * 0.35,
    vy = -rng(0.2, 1.0) * (params.rise or 70) * 0.25,
    r = r0,
    r0 = r0,
    r1 = r0 * (params.grow or 1.2),
    life = life,
    max_life = life,
    phase = rng(0.0, 6.2831853),
  }
  while #bubbles > 400 do
    table.remove(bubbles, 1)
  end
end

function step(dt, t, params)
  elapsed = t
  if #bubbles == 0 then return end
  local rise = params.rise or 70
  local drift = params.drift or 14
  local keep = {}
  for i = 1, #bubbles do
    local b = bubbles[i]
    b.life = b.life - dt
    if b.life > 0 then
      -- 向上"加速"（用持续朝上的推力实现）
      b.vy = b.vy - rise * dt
      -- 横向摆动：正弦漂移，让气泡不是笔直上升
      b.vx = b.vx + math.sin(t * 2.4 + b.phase) * drift * dt
      b.x = b.x + b.vx * dt
      b.y = b.y + b.vy * dt
      -- 边浮边长大
      local grown = 1.0 - b.life / b.max_life
      b.r = b.r0 + (b.r1 - b.r0) * grown
      keep[#keep + 1] = b
    end
  end
  bubbles = keep
end

function draw(ctx, params)
  if #bubbles == 0 then return end
  local opacity = params.opacity or 0.9
  local sw = params.stroke or 2
  local fr, fg, fb, fa = hex_to_rgb(params.fill or "#E6F1F7")
  local lr, lg, lb, la = hex_to_rgb(params.line or "#3A92C5")

  for i = 1, #bubbles do
    local b = bubbles[i]
    local k = b.life / b.max_life
    -- 冒头时快速淡入（0.08s），之后随寿命线性淡出——否则气泡会"啪"地出现
    local in_k = (1.0 - k) * b.max_life / 0.08
    if in_k > 1.0 then in_k = 1.0 end
    local a = math.floor(opacity * k * in_k * 255)
    if a > 0 and b.r > 0.5 then
      -- 描边圆 + 极淡的填充 + 左上角一点高光
      ctx:ring(b.x, b.y, b.r, sw, lr, lg, lb, math.floor(a * la / 255))
      ctx:circle(b.x, b.y, math.max(0.5, b.r - sw * 0.5), fr, fg, fb, math.floor(a * 0.22 * fa / 255))
      ctx:circle(b.x - b.r * 0.35, b.y - b.r * 0.35, math.max(0.5, b.r * 0.16), 255, 255, 255,
        math.floor(a * 0.8 * fa / 255))
    end
  end
end
