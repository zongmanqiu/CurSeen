-- 雪花旋落：光标经过的地方飘下小小的六角雪花，边落边自转、边落边缩小，飘到寿尽就化掉。
--
-- 雪花用**线段现画**——6 条主枝，每条主枝上再斜出两对侧枝，顶点按 (支长, 夹角) 实时算出来。
--
-- ★ 与 `pixie_dust`（像素尘埃）同属"下落粒子"族，差别在**形状清晰 + 自转 + 随寿命收缩**：
--   尘埃是不定型的小点向下掉，这套是"有骨架的晶体"一边转一边变小。
--
-- ★ 空闲归零：雪花全部化掉后停止绘制（宿主按"有没有产出指令"判活），静止 0 开销。

effect_name = "雪花旋落"
name = "雪花旋落"
role = "effect"
function schema()
  return {
    { key = "opacity",     label = "不透明度",   type = "float", min = 0.05, max = 1,   step = 0.01, default = 0.9 },
    { key = "interval_ms", label = "生成间隔",   type = "float", min = 20,   max = 600, step = 10,   default = 140 },
    { key = "spacing",     label = "最小间距",   type = "float", min = 4,    max = 120, step = 2,    default = 28 },
    { key = "life",        label = "存活时间",   type = "float", min = 0.3,  max = 6,   step = 0.1,  default = 1.6 },
    { key = "size",        label = "雪花大小",   type = "float", min = 3,    max = 48,  step = 1,    default = 14 },
    { key = "fall_speed",  label = "下落速度",   type = "float", min = 5,    max = 300, step = 5,    default = 60 },
    { key = "sway",        label = "横向飘摆",   type = "float", min = 0,    max = 120, step = 2,    default = 26 },
    { key = "spin_speed",  label = "自转速度",   type = "float", min = -360, max = 360, step = 10,   default = 110 },
    { key = "shrink_to",   label = "缩到多小",   type = "float", min = 0.05, max = 1,   step = 0.05, default = 0.35 },
    { key = "stroke",      label = "线条粗细",   type = "float", min = 0.4,  max = 4,   step = 0.1,  default = 1.2 },
    { key = "color",       label = "颜色",       type = "color", default = "#E8F4FF" },
  }
end

local ARMS = 6                                     -- 六角
local ARM_STEP = 2.0 * math.pi / ARMS              -- 每支间隔 60°
local SIDE_AT = { 0.50, 0.78 }                     -- 侧枝在主枝上的着生位置（占枝长比例）
local SIDE_ANG = 0.96                              -- 侧枝与主枝的夹角（弧度，≈55°）
local SIDE_LEN = 0.30                              -- 最内侧侧枝的长度（占枝长比例）
local FADE_FROM = 0.75                             -- 寿命走过这个比例后开始淡出

flakes = {}
next_spawn = 0.0
last_x, last_y = nil, nil
elapsed = 0.0

local function draw_flake(ctx, f, r, g, b, a_u8, stroke, shrink_to)
  local prog = 1.0 - f.life / f.max_life          -- 0 → 1，走完即寿尽
  -- 尺寸随寿命线性收缩：起手最大，消失时只剩 shrink_to 那么点
  local sc = 1.0 - (1.0 - shrink_to) * prog
  local L = f.size * sc
  if L < 0.8 then return end

  local alpha = a_u8
  if prog > FADE_FROM then
    alpha = math.floor(a_u8 * (1.0 - (prog - FADE_FROM) / (1.0 - FADE_FROM)))
  end
  if alpha < 1 then return end

  local w = stroke * sc
  if w < 0.5 then w = 0.5 end
  local w_side = w * 0.75
  if w_side < 0.4 then w_side = 0.4 end

  for k = 0, ARMS - 1 do
    local a = f.spin + k * ARM_STEP
    local ux, uy = math.cos(a), math.sin(a)
    -- 主枝：中心 → 枝尖
    ctx:line(f.x, f.y, f.x + ux * L, f.y + uy * L, w, r, g, b, alpha)
    -- 侧枝：着生点两侧各斜出一根，越靠外的着生点侧枝越短
    for s = 1, #SIDE_AT do
      local rr = SIDE_AT[s]
      local px, py = f.x + ux * L * rr, f.y + uy * L * rr
      local sl = L * (SIDE_LEN - rr * 0.12)
      for dir = -1, 1, 2 do
        local sa = a + dir * SIDE_ANG
        ctx:line(px, py, px + math.cos(sa) * sl, py + math.sin(sa) * sl,
                 w_side, r, g, b, alpha)
      end
    end
  end
end

function init()
  flakes = {}
  next_spawn = 0.0
  last_x, last_y = nil, nil
  elapsed = 0.0
end

-- 在 (x, y) 落一片雪花；个体差异（大小 / 摆频 / 相位 / 自转方向）全部随机
local function add_flake(x, y, t, params)
  local life = params.life or 1.6
  if life < 0.3 then life = 0.3 end
  local spin = rng(0.0, 2.0 * math.pi)
  local speed = params.spin_speed or 110
  flakes[#flakes + 1] = {
    x = x, y = y,
    life = life,
    max_life = life,
    size = (params.size or 14) * rng(0.7, 1.25),
    spin = spin,
    spin_speed = speed * rng(0.7, 1.3),           -- 每片转速略不同，群体不齐步
    sway_phase = rng(0.0, 2.0 * math.pi),
    sway_freq = rng(1.1, 2.3),
  }
end

function on_move(x, y, t, params)
  elapsed = t
  -- 拖尾会给密集的插值点：用"最小间距 + 生成间隔"两道闸，既不缺片也不叠成一坨
  local interval = (params.interval_ms or 140) / 1000.0
  local spacing = params.spacing or 28

  local far_enough = true
  if last_x then
    local dx, dy = x - last_x, y - last_y
    far_enough = (dx * dx + dy * dy) >= spacing * spacing
  end
  if far_enough and (not last_x or t - next_spawn >= interval) then
    add_flake(x, y, t, params)
    last_x, last_y = x, y
    next_spawn = t
  end
end

function step(dt, t, params)
  elapsed = t
  if #flakes == 0 then return end

  local fall = params.fall_speed or 60
  local sway = params.sway or 26

  -- 倒序删除，避免搬移后漏掉元素
  for i = #flakes, 1, -1 do
    local f = flakes[i]
    f.life = f.life - dt
    if f.life <= 0.0 then
      table.remove(flakes, i)
    else
      f.y = f.y + fall * dt
      -- 横向：正弦摆（左右飘）+ 自转独立推进
      f.x = f.x + math.sin(t * f.sway_freq + f.sway_phase) * sway * dt
      f.spin = f.spin + f.spin_speed * dt * math.pi / 180.0   -- 参数是"度/秒"
    end
  end
end

function draw(ctx, params)
  if #flakes == 0 then return end
  local r, g, b, ca = hex_to_rgb(params.color or "#E8F4FF")
  local a_u8 = math.floor((params.opacity or 0.9) * 255 * ca / 255)
  if a_u8 < 1 then return end
  local stroke = params.stroke or 1.2
  local shrink_to = params.shrink_to or 0.35

  for i = 1, #flakes do
    draw_flake(ctx, flakes[i], r, g, b, a_u8, stroke, shrink_to)
  end
end
