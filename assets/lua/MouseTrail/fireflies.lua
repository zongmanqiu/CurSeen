-- 流萤：一群发光的小光点在屏幕上各自游荡；鼠标靠近时，**离光标最近的那只**先去追随，
-- 其余光点再依次追随"离自己最近的已追随者"——于是沿光标的去向**链式传染**成一条流动的光带；
-- 鼠标停一会儿，光带散开、光点淡出。
--
-- ★ 与本目录其它特效的区别：这是唯一一个"**有自主行为**"的——没被招募的光点不等鼠标，
--   自己到处游荡。所以它比别的特效多一个"停笔"开关（idle_s），否则鼠标静止也会永远出帧。
--
-- ★ 空闲归零：鼠标停下 idle_s 秒后光点淡出并**停止绘制**——宿主按"这一帧有没有产出指令"
--   判活（`modules/trail/effects/lua_effect.rs`），停画即停帧，静止时 0 开销；
--   再次移动会在光标附近重新放一群。

effect_name = "流萤"
name = "流萤"
role = "effect"
function schema()
  return {
    { key = "opacity",      label = "不透明度",   type = "float", min = 0.05, max = 1,   step = 0.01, default = 0.9 },
    { key = "count",        label = "光点数量",   type = "int",   min = 2,    max = 30,  step = 1,    default = 12 },
    { key = "size",         label = "光点大小",   type = "float", min = 0.5,  max = 8,   step = 0.1,  default = 2.4 },
    { key = "follow_range", label = "追随半径",   type = "float", min = 20,   max = 220, step = 5,    default = 60 },
    { key = "speed",        label = "游走速度",   type = "float", min = 12,   max = 240, step = 2,    default = 72 },
    { key = "spawn_radius", label = "出场范围",   type = "float", min = 40,   max = 600, step = 10,   default = 110 },
    { key = "idle_s",       label = "停下后淡出", type = "float", min = 0.3,  max = 6,   step = 0.1,  default = 2.0 },
    { key = "tail_len",     label = "尾缀长度",   type = "float", min = 0,    max = 30,  step = 0.5,  default = 9 },
    { key = "twinkle",      label = "闪烁强度",   type = "float", min = 0,    max = 1,   step = 0.05, default = 0.45 },
    { key = "color",        label = "颜色",       type = "color", default = "#FFD98A" },
  }
end

-- 光晕相对内核的半径倍数（加色三层叠出来的柔光范围）
local HALO_SCALE = 2.2
-- 内核提亮系数：0 = 用本色，1 = 纯白。取 0.55 让中心"烧"起来但保留色相
local CORE_LIFT = 0.55
-- 尾缀分几段画（段越宽越软；2 段足够看出"拖出来的一缕光"）
local TAIL_SEGS = 2

flies = {}
awake = false
elapsed = 0.0
last_move_t = 0.0
cx, cy = 0.0, 0.0

-- 把"按 60fps 调的手感系数 k"折算成本帧等效值，使行为与帧率无关
local function rate(k, dt)
  local n = dt * 60.0
  if n > 3.0 then n = 3.0 end
  if n <= 0.0 then return 0.0 end
  return 1.0 - (1.0 - k) ^ n
end

-- 乘性衰减的同样折算（k 是"每帧保留多少"）
local function decay(k, dt)
  local n = dt * 60.0
  if n > 3.0 then n = 3.0 end
  if n <= 0.0 then return 1.0 end
  return k ^ n
end

-- 在光标周围放射状撒一群光点（半径开方 → 圆内均匀，不会全挤在外圈）
--
-- ★ 第一只**强制贴光标**（8px 内）：链式传染需要一个"起点"——只有离光标最近的那只先被
--   招募，其余才顺着它传染开。如果全群随机撒、恰好没有一只落进 `follow_range`，
--   整群就永远不会跟随（默认参数实测：12 只撒在 240px 半径里、最近的一只离光标 248px，
--   现象就是"光点只在乱走、完全不理会鼠标"）。留一只贴边，传染链才有头。
--   —— 同理，出场范围默认值也从 240 收到 110：撒得越散，平均间距越大，
--   "相邻 < follow_range"越难成立（光点间也要够近才传染得动）。
local function spawn(count, size_base, radius)
  flies = {}
  local sw = screen_w or 0
  local sh = screen_h or 0
  for i = 1, count do
    local ang = rng(0.0, 2.0 * math.pi)
    local rr
    if i == 1 then
      rr = rng(0.0, 8.0)                       -- 保底的那只：贴光标
    else
      rr = math.sqrt(rng(0.0, 1.0)) * radius
    end
    local x = cx + math.cos(ang) * rr
    local y = cy + math.sin(ang) * rr
    -- ★ screen_w/screen_h 每帧由宿主刷新；首帧可能还是 0，那时先不夹（step 里会再夹一次）
    if sw > 20.0 then x = clamp(x, 5.0, sw - 5.0) end
    if sh > 20.0 then y = clamp(y, 5.0, sh - 5.0) end
    local dir = rng(0.0, 2.0 * math.pi)
    flies[i] = {
      x = x, y = y,
      vx = 0.0, vy = 0.0,
      angle = dir,
      wander = dir,
      -- 每只的光学差异：呼吸相位 + 呼吸快慢 + 明暗系数，让整群不会"齐闪"
      phase = rng(0.0, 2.0 * math.pi),
      tempo = rng(0.7, 1.35),
      tint = rng(0.72, 1.0),
      size = size_base * rng(0.75, 1.3),
      chasing = false,
      target = nil,
    }
  end
end

local function draw_fly(ctx, f, r, g, b, a_u8, tail_len, twinkle, t)
  -- 呼吸：亮度在 [1-twinkle, 1] 之间波动（twinkle=0 时恒亮）
  local breathe = 1.0 - twinkle * 0.5 * (1.0 - math.sin(t * 3.1 * f.tempo + f.phase))
  local ai = math.floor(a_u8 * breathe * f.tint)
  if ai > 255 then ai = 255 end
  if ai < 1 then return end

  local rad = f.size * (0.8 + 0.35 * breathe)

  -- 尾缀：朝运动反方向拉出渐细的两段（按速度方向取单位向量）
  if tail_len > 0.1 then
    local spd = math.sqrt(f.vx * f.vx + f.vy * f.vy)
    if spd > 1.0 then
      local ux, uy = -f.vx / spd, -f.vy / spd
      local seg = tail_len / TAIL_SEGS
      for k = 1, TAIL_SEGS do
        local t0 = (k - 1) * seg
        local t1 = k * seg
        -- 越靠后越细：宽度按 1.0 → 0.35 线性收
        local w = rad * (1.0 - 0.65 * (k - 1) / TAIL_SEGS) * 0.9
        if w < 0.4 then w = 0.4 end
        ctx:line(f.x + ux * t0, f.y + uy * t0, f.x + ux * t1, f.y + uy * t1,
                 w, r, g, b, math.floor(ai * 0.42))
      end
    end
  end

  -- 光晕（加色，低 alpha）——柔光范围由 HALO_SCALE 定
  ctx:glow(f.x, f.y, rad * HALO_SCALE, r, g, b, math.floor(ai * 0.5))

  -- 内核：朝白色提亮，让中心看起来是"光源"而不是一块色斑
  local cr = math.floor(r + (255.0 - r) * CORE_LIFT)
  local cg = math.floor(g + (255.0 - g) * CORE_LIFT)
  local cb = math.floor(b + (255.0 - b) * CORE_LIFT)
  ctx:circle(f.x, f.y, rad, cr, cg, cb, ai)
end

function init()
  flies = {}
  awake = false
  elapsed = 0.0
  last_move_t = 0.0
end

function on_move(x, y, t, params)
  elapsed = t
  cx, cy = x, y
  last_move_t = t
  if not awake then
    -- 醒来的第一件事：在光标附近重新放一群（上次淡出后光点已经不在了）
    awake = true
    local n = math.floor(params.count or 12)
    if n < 2 then n = 2 end
    spawn(n, params.size or 2.4, params.spawn_radius or 110)
  end
end

function step(dt, t, params)
  elapsed = t
  if not awake or #flies == 0 then return end

  local idle_s = params.idle_s or 2.0
  if t - last_move_t > idle_s then
    awake = false          -- 已淡完，收队；下次移动再重新放一群
    return
  end

  -- 数量被改了就就地重建（参数变更本来会走 init，这里兜住手动改的情况）
  local want = math.floor(params.count or 12)
  if want < 2 then want = 2 end
  if want ~= #flies then
    spawn(want, params.size or 2.4, params.spawn_radius or 110)
  end

  local follow = params.follow_range or 60
  local speed = params.speed or 72
  local n = #flies

  -- 1) 每帧重新招募（鼠标一走开，队伍自然散掉）
  for i = 1, n do
    local f = flies[i]
    f.chasing = false
    f.target = nil
  end

  -- 2) 只有"离光标最近、且在追随半径内"的那一只直接追光标
  local head, head_d = nil, follow
  for i = 1, n do
    local f = flies[i]
    local dx, dy = f.x - cx, f.y - cy
    local d = math.sqrt(dx * dx + dy * dy)
    if d < head_d then
      head_d = d
      head = f
    end
  end
  local queue = {}
  if head then
    head.chasing = true
    queue[1] = head
  end

  -- 3) 链式传染：其余光点追随"离自己最近的已追随者"；每收编一只就多一个传染源，
  --    反复扫描到某一轮没有新成员为止 —— "光带"就是这么长出来的
  local added = true
  while added do
    added = false
    for i = 1, n do
      local f = flies[i]
      if not f.chasing then
        local nearest, nd = nil, follow
        for k = 1, #queue do
          local src = queue[k]
          local dx, dy = f.x - src.x, f.y - src.y
          local d = math.sqrt(dx * dx + dy * dy)
          if d < nd then
            nd = d
            nearest = src
          end
        end
        if nearest then
          f.chasing = true
          f.target = nearest
          queue[#queue + 1] = f
          added = true
        end
      end
    end
  end

  local k_turn = rate(0.30, dt)
  local k_acc = rate(0.20, dt)
  local k_wan = rate(0.16, dt)               -- 未被招募时的向心/游荡响应（比追随转向稍快）
  local k_stop = decay(0.80, dt)
  local sw = screen_w or 0
  local sh = screen_h or 0

  for i = 1, n do
    local f = flies[i]
    if f.chasing then
      local tx, ty = cx, cy
      if f.target then tx, ty = f.target.x, f.target.y end
      local dx, dy = tx - f.x, ty - f.y
      local d = math.sqrt(dx * dx + dy * dy)
      if d > 8.0 then
        local ta = math.atan(dy, dx)          -- ★ lua54 没有 math.atan2
        local diff = ta - f.angle
        while diff > math.pi do diff = diff - 2.0 * math.pi end
        while diff < -math.pi do diff = diff + 2.0 * math.pi end
        f.angle = f.angle + diff * k_turn
        local sp = speed * 1.8
        local near = d * 9.0                  -- 越近越慢，避免在目标上over-shoot
        if near < sp then sp = near end
        f.vx = f.vx + (math.cos(f.angle) * sp - f.vx) * k_acc
        f.vy = f.vy + (math.sin(f.angle) * sp - f.vy) * k_acc
      else
        f.vx = f.vx * k_stop
        f.vy = f.vy * k_stop
      end
    else
      -- 没被招募：**绕着光标游荡**——朝光标的向心漂移 + 随机摆动。
      --   纯随机游走会让光点飘到天边，光标一移动就再也招募不到；有了向心力，
      --   它们才像"围着人的萤火"，鼠标一动总有光点进入追随半径、把链条接上。
      f.wander = f.wander + rng(-0.5, 0.5) * 0.3
      local dx, dy = cx - f.x, cy - f.y
      local d = math.sqrt(dx * dx + dy * dy)
      local tvx, tvy
      if d > 4.0 then
        local pull = speed * 0.8               -- 向心速度
        local wob = speed * 0.35               -- 随机摆动（"游荡感"来自它）
        tvx = (dx / d) * pull + math.cos(f.wander) * wob
        tvy = (dy / d) * pull + math.sin(f.wander) * wob
      else
        tvx = math.cos(f.wander) * speed * 0.35
        tvy = math.sin(f.wander) * speed * 0.35
      end
      f.vx = f.vx + (tvx - f.vx) * k_wan
      f.vy = f.vy + (tvy - f.vy) * k_wan
      if sw > 20.0 and sh > 20.0 then
        if f.x < 20.0 then f.wander = 0.0 end
        if f.x > sw - 20.0 then f.wander = math.pi end
        if f.y < 20.0 then f.wander = math.pi * 0.5 end
        if f.y > sh - 20.0 then f.wander = -math.pi * 0.5 end
      end
    end

    f.x = f.x + f.vx * dt
    f.y = f.y + f.vy * dt
    local spd = math.sqrt(f.vx * f.vx + f.vy * f.vy)
    if spd > 0.1 then f.angle = math.atan(f.vy, f.vx) end
    if sw > 20.0 then f.x = clamp(f.x, 5.0, sw - 5.0) end
    if sh > 20.0 then f.y = clamp(f.y, 5.0, sh - 5.0) end
  end
end

function draw(ctx, params)
  if not awake or #flies == 0 then return end

  local idle_s = params.idle_s or 2.0
  local idle = elapsed - last_move_t
  -- 后 40% 的 idle 时间里整体淡出，到点正好归零（而不是"啪"地消失）
  local fade = 1.0
  local from = idle_s * 0.6
  if idle > from and idle_s > from then
    fade = 1.0 - (idle - from) / (idle_s - from)
    if fade < 0.0 then fade = 0.0 end
  end
  if fade <= 0.02 then return end

  local a_u8 = math.floor((params.opacity or 0.9) * fade * 255)
  if a_u8 < 1 then return end

  -- ★ hex_to_rgb 返回 **4** 个值（r,g,b,a）；这里只要前 3 个，逐个接住即可
  local r, g, b = hex_to_rgb(params.color or "#FFD98A")

  local tail_len = params.tail_len or 9
  local twinkle = params.twinkle or 0.45

  for i = 1, #flies do
    draw_fly(ctx, flies[i], r, g, b, a_u8, tail_len, twinkle, elapsed)
  end
end
