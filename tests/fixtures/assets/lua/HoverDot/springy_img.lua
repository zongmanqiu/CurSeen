-- 弹簧挂件：一串挂珠沿弹簧物理串在光标下面，跑动时甩尾、静止时垂挂晃荡。
--
-- ★★ 为什么 `draw` 要返回续帧毫秒数（2026-11-09 修）：
--   悬浮标的帧是**按需**的——指针不动就不出帧（静止零功耗那条既有语义）。
--   而这条链的摆动是**时间驱动**的：弹簧还在把珠子拉回平衡位，没有帧就等于把摆到一半的
--   链子钉在半空。用户报的现象正是它：「鼠标不动，球就停止，很诡异」——不是物理坏了，
--   是宿主没再要帧。所以链子还在动时 `draw` 返回 ≈60fps 的间隔让宿主继续出帧；
--   收敛到静止后不再返回，帧门自动睡回去（静止时依然是 0 功耗）。
--
-- ★ 图片样式已删除（2026-11-09）：面板的「图片」浏览行**只在「图片」类型（stem=png）
--   下出现**（见 `ui/pages/trail.rs` 的 `visible_when`），其它悬浮标类型根本拿不到图，
--   于是"图片"那一档永远只画出一片空白。参数一共三档（样式 / 图片缩放 / 独立不透明度）
--   一并删掉：不透明度本来就在颜色里给（`alpha = true`，与实心圆 / 圆环 / 激光笔同规格）。
--   文件名仍叫 springy_img（历史名）：id = 文件名，改名会让用户已保存的启用列表失效。

effect_name = "弹簧挂件"
name = "弹簧挂件"
role = "effect"

function schema()
  return {
    { key = "count",   label = "节数",     type = "int",   min = 2,  max = 10,   step = 1,  default = 5 },
    { key = "seg",     label = "段长",     type = "int",   min = 10, max = 200,  step = 1,  default = 36 },
    { key = "stiff",   label = "弹性",     type = "float", min = 10, max = 200,  step = 5,  default = 60 },
    { key = "gravity", label = "重力",     type = "float", min = 0,  max = 2000, step = 50, default = 600 },
    { key = "size",    label = "圆珠半径", type = "float", min = 4,  max = 40,   step = 1,  default = 12 },
    { key = "color",   label = "圆珠颜色", type = "color", alpha = true, default = "#00C2D8" },
  }
end

-- 状态：质点链（1 号钉在光标上，其余靠弹簧牵引）
local nodes = nil
-- 本帧链子是否还在动（决定要不要续帧）
local moving = true

local function ensure(n)
  if nodes ~= nil and #nodes == n then
    return
  end
  nodes = {}
  for i = 1, n do
    nodes[i] = { x = cursor_x, y = cursor_y + (i - 1) * 20, vx = 0.0, vy = 0.0 }
  end
  -- 刚重建 = 离平衡位很远，必须申请帧把它摆过去。
  moving = true
end

function init()
  nodes = nil
  moving = true
end

function step(dt, t, params)
  local n = params.count or 5
  local seg = params.seg or 36
  local stiff = params.stiff or 60
  local grav = params.gravity or 600
  ensure(n)

  -- 1 号钉在指针上
  nodes[1].x, nodes[1].y = cursor_x, cursor_y

  local damp = math.exp(-dt * 2.5)   -- 阻尼：每秒衰减到 e^-2.5 ≈ 8%
  local margin = seg * 0.25          -- 屏幕边缘反弹的留边（圆珠半径的粗略近似）
  local moved = 0.0
  for i = 2, n do
    local cur = nodes[i]
    local prev = nodes[i - 1]
    -- 弹簧：加速度 = 弹性 × (当前段长 - 自然段长)，方向指向上一节
    local dx, dy = prev.x - cur.x, prev.y - cur.y
    local d = math.sqrt(dx * dx + dy * dy)
    if d > 0.001 then
      local f = stiff * (d - seg) / d
      cur.vx = cur.vx + dx * f * dt
      cur.vy = cur.vy + dy * f * dt
    end
    cur.vy = cur.vy + grav * dt
    cur.vx = cur.vx * damp
    cur.vy = cur.vy * damp
    local ox, oy = cur.x, cur.y
    cur.x = cur.x + cur.vx * dt
    cur.y = cur.y + cur.vy * dt
    -- 屏幕边缘反弹（恢复系数 0.6）
    if cur.x < margin then
      cur.x, cur.vx = margin, -cur.vx * 0.6
    elseif cur.x > screen_w - margin then
      cur.x, cur.vx = screen_w - margin, -cur.vx * 0.6
    end
    if cur.y < margin then
      cur.y, cur.vy = margin, -cur.vy * 0.6
    elseif cur.y > screen_h - margin then
      cur.y, cur.vy = screen_h - margin, -cur.vy * 0.6
    end
    moved = moved + math.abs(cur.x - ox) + math.abs(cur.y - oy)
  end

  -- 静止判定用**本帧位移**而不是速度：速度在平衡位会有一点点数值残差，位移则直接
  -- 等于"这一帧看不看得出来在动"。0.5px/帧 已经低于人眼分辨，比它小就收工。
  moving = moved > 0.5
end

-- 尾部轻微淡出的 alpha：链的方向感更清楚；base = 颜色自带的 alpha（0–255）
local function fade_alpha(i, n, base)
  local fade = 1 - 0.35 * (i - 1) / math.max(1, n - 1)
  return math.floor(math.max(0, math.min(255, base * fade + 0.5)))
end

function draw(ctx, params)
  local n = params.count or 5
  ensure(n)
  local br, bg, bb, ba = hex_to_rgb(params.color or "#00C2D8")
  local size = params.size or 12
  for i = 1, n do
    local a = fade_alpha(i, n, ba)
    if i == 1 then
      -- 挂点只画环：视觉上是"钩住光标"，不挡指针
      ctx:ring(nodes[i].x, nodes[i].y, size, 2, br, bg, bb, a)
    else
      ctx:circle(nodes[i].x, nodes[i].y, size, br, bg, bb, a)
      ctx:ring(nodes[i].x, nodes[i].y, size + 2, 1.5, br, bg, bb, a)
    end
  end
  -- 还在摆就约 60fps 再来一帧；摆完不再返回 → 宿主睡回静止态。
  if moving then
    return 16
  end
end
