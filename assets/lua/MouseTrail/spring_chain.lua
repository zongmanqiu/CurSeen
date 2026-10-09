-- 弹簧珠链：三条**滞后不同**的弹簧链同时追光标，每条由一串径向渐变小球串成"珠链"，
-- 沿链向尾部收细，链身随正弦轻微摆动；鼠标停下后三股链子会自己收拢、淡出。
--
-- 三条链滞后 0.30 / 0.40 / 0.50（色品红 / 绿 / 紫），节点是径向渐变球、强制最小段距 3.5px。
-- 每个球用 `radial()` 画；节点数取 14 是为了控制每帧指令数。
--
-- ★ 空闲归零：三条链都收拢到光标附近后**停止绘制**——宿主按"这一帧有没有产出指令"判活
--   （`modules/trail/effects/lua_effect.rs`），停画即停帧，静止时 0 开销；
--   鼠标再动会自动唤醒。

effect_name = "弹簧珠链"
name = "弹簧珠链"
role = "effect"
function schema()
  return {
    { key = "opacity",    label = "不透明度", type = "float", min = 0.05, max = 1,   step = 0.01, default = 0.85 },
    { key = "node_count", label = "每链节点数", type = "int", min = 4,    max = 24,  step = 1,    default = 14 },
    { key = "head_radius", label = "头部球半径", type = "float", min = 1, max = 20,  step = 0.5,  default = 5 },
    { key = "taper",      label = "尾部收细", type = "float", min = 0,    max = 0.95,step = 0.05, default = 0.75 },
    { key = "lag_scale",  label = "跟随速度", type = "float", min = 0.1,  max = 2,   step = 0.05, default = 1.0 },
    { key = "sway",       label = "摆动幅度", type = "float", min = 0,    max = 20,  step = 0.5,  default = 5 },
    { key = "sway_rate",  label = "摆动频率", type = "float", min = 0,    max = 6,   step = 0.1,  default = 2.2 },
    { key = "idle_s",     label = "收拢后停笔", type = "float", min = 0,   max = 3,   step = 0.1,  default = 0.35 },
    { key = "color1",     label = "链1颜色", type = "color",   default = "#F967FB" },
    { key = "color2",     label = "链2颜色", type = "color",   default = "#53BC28" },
    { key = "color3",     label = "链3颜色", type = "color",   default = "#6958D5" },
  }
end

-- 三条链的滞后系数（0.30 / 0.40 / 0.50，越靠后越"懒"）
local CHAIN_LAGS = { 0.30, 0.40, 0.50 }
-- 节点间最小距离 3.5px：不加它，鼠标慢速时整条链会缩成一个点
local MIN_SEG = 3.5

chains = {}
node_count = 0
elapsed = 0.0
cx, cy = 0.0, 0.0
has_cursor = false
settled_for = 0.0

local function rebuild(n)
  chains = {}
  for c = 1, #CHAIN_LAGS do
    local nodes = {}
    for i = 1, n do
      nodes[i] = { x = cx, y = cy }
    end
    chains[c] = nodes
  end
  node_count = n
end

function init()
  chains = {}
  node_count = 0
  elapsed = 0.0
  has_cursor = false
  settled_for = 0.0
end

function on_move(x, y, t, params)
  elapsed = t
  cx, cy = x, y
  if not has_cursor then
    has_cursor = true
    rebuild(math.floor(params.node_count or 14))
  end
end

function step(dt, t, params)
  elapsed = t
  if not has_cursor then return end

  local want = math.floor(params.node_count or 14)
  if want < 4 then want = 4 end
  if want ~= node_count then
    rebuild(want)
  end

  local lag_scale = params.lag_scale or 1.0
  local moved = 0.0

  for c = 1, #chains do
    local nodes = chains[c]
    local lag = CHAIN_LAGS[c] * lag_scale
    if lag > 1.0 then lag = 1.0 end
    for i = 1, #nodes do
      local nd = nodes[i]
      -- 目标：头节点追光标，其余节点追**前一个节点**（链式弹簧的关键：滞后逐级传递）
      local tx, ty
      if i == 1 then
        tx, ty = cx, cy
      else
        tx, ty = nodes[i - 1].x, nodes[i - 1].y
      end
      local nx = nd.x + (tx - nd.x) * lag
      local ny = nd.y + (ty - nd.y) * lag
      -- 强制最小段距：不足就往"背离前一点"的方向推开
      if i > 1 then
        local px, py = nodes[i - 1].x, nodes[i - 1].y
        local dx, dy = nx - px, ny - py
        local d = math.sqrt(dx * dx + dy * dy)
        if d < MIN_SEG then
          if d < 0.0001 then
            nx, ny = px + MIN_SEG, py
          else
            nx = px + dx / d * MIN_SEG
            ny = py + dy / d * MIN_SEG
          end
        end
      end
      moved = moved + math.abs(nx - nd.x) + math.abs(ny - nd.y)
      nd.x, nd.y = nx, ny
    end
  end

  -- 收拢判定：全部节点几乎不动了，就开始计时；超过 idle_s 就停笔（0 开销）
  if moved < 0.4 then
    settled_for = settled_for + dt
  else
    settled_for = 0.0
  end
end

function draw(ctx, params)
  if not has_cursor or node_count == 0 then return end
  local idle_s = params.idle_s or 0.35
  if settled_for > idle_s then return end

  local opacity = params.opacity or 0.85
  -- 收拢后再淡出，避免"啪"地一下整体消失
  local fade_k = 1.0
  if idle_s > 0 and settled_for > idle_s * 0.5 then
    fade_k = 1.0 - (settled_for - idle_s * 0.5) / (idle_s * 0.5)
    if fade_k < 0.0 then fade_k = 0.0 end
  end
  if fade_k <= 0.01 then return end

  -- ★ 必须逐个解出 r,g,b —— `hex_to_rgb` 返回 **4** 个值，若写成
  --   `{ hex_to_rgb(a), hex_to_rgb(b), hex_to_rgb(c) }`，Lua 只会把最后一个调用展开成
  --   4 项（前两个被截成 1 项），于是 colors[3] 拿到的其实是"红色通道"，g/b 全是 nil。
  local r1, g1, b1 = hex_to_rgb(params.color1 or "#F967FB")
  local r2, g2, b2 = hex_to_rgb(params.color2 or "#53BC28")
  local r3, g3, b3 = hex_to_rgb(params.color3 or "#6958D5")
  local colors = { { r1, g1, b1 }, { r2, g2, b2 }, { r3, g3, b3 } }
  local head_r = params.head_radius or 5
  local taper = params.taper or 0.75
  local sway = params.sway or 5
  local sway_rate = params.sway_rate or 2.2

  for c = 1, #chains do
    local nodes = chains[c]
    -- ★ 表**不是**多值：`local r, g, b = colors[c]` 只会让 r 拿到整张表、g/b 为 nil，
    --   于是 radial 的颜色参数变成「一个表 + 两个 nil」（2026-09-27 指令流验收抓到的真 bug）。
    --   必须逐项取下标。
    local cc = colors[c]
    local r, g, b = cc[1], cc[2], cc[3]
    local n = #nodes
    -- 从尾画到头，头节点压在最上面
    for i = n, 1, -1 do
      local nd = nodes[i]
      local ratio = 1.0 - (i - 1) / n * taper          -- 头部最粗、尾部收细
      local rad = head_r * ratio
      if rad < 0.5 then rad = 0.5 end

      -- 正弦摆动：沿本段法线方向上摆（视觉层，不回写物理位置）
      local ox, oy = 0.0, 0.0
      if sway > 0 then
        local a = nodes[math.max(1, i - 1)]
        local bx = nodes[math.min(n, i + 1)]
        local dx, dy = bx.x - a.x, bx.y - a.y
        local d = math.sqrt(dx * dx + dy * dy)
        if d > 0.001 then
          local off = math.sin(elapsed * sway_rate + i * 0.7) * sway * (i / n)
          ox, oy = -dy / d * off, dx / d * off
        end
      end

      local a_u8 = math.floor(opacity * fade_k * (0.35 + 0.65 * ratio) * 255)
      ctx:radial(nd.x + ox, nd.y + oy, rad, r, g, b, a_u8)
      -- 头节点再加一层辉光，让它"亮起来"
      if i == 1 then
        ctx:glow(nd.x + ox, nd.y + oy, rad * 0.9, r, g, b,
          math.floor(opacity * fade_k * 0.8 * 255))
      end
    end
  end
end
