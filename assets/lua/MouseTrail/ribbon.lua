-- 流光飘带：精确复刻原版 Python ribbon.py（轨迹类）。沿鼠标轨迹构建一条**填充多边形**
-- 飘带：宽度沿轨迹三次方收窄（尾尖→头部最宽）、左右边界 3 点滑动平均平滑，
-- 头部以半径=半带宽的**完整圆**收口（与原版 `path.united(cap)` 等价，见 build_path）。
-- 整条带单色（彩虹整体随时间旋转，非逐点渐变）。鼠标停住后尾迹按「尾迹长度」褪去，
-- 但头部那颗圆会一直留在鼠标处。

effect_name = "流光飘带"
name = "流光飘带"
role = "effect"
function schema()
  return {
    { key = "opacity",       label = "不透明度", type = "float",  min = 0.05, max = 1,  step = 0.01, default = 0.5 },
    { key = "max_width",     label = "带宽",     type = "int",    min = 2,    max = 40, step = 1,    default = 20 },
    { key = "fade_duration", label = "尾迹长度", type = "float",  min = 0.2,  max = 3,  step = 0.1,  default = 1.0 },
    { key = "rainbow",       label = "彩虹色",   type = "bool",  default = true },
    { key = "fixed_color",   label = "固定色",   type = "color",  default = "#FF0000" },
  }
end

local MIN_DIST = 2.0
local MAX_POINTS = 120
-- 头部整圆的细分数：越多越接近正圆。24 段时最大带宽（40→半径 20）下的弦高误差 0.17px。
local HEAD_SEGMENTS = 24
local TAU = 2.0 * math.pi

points = {}
elapsed = 0

function init()
  points = {}
  elapsed = 0
end

local function add_point(x, y, t)
  local last = points[#points]
  if last then
    local dx, dy = x - last.x, y - last.y
    if math.sqrt(dx * dx + dy * dy) < MIN_DIST then
      return
    end
  end
  points[#points + 1] = { x = x, y = y, t = t }
  while #points > MAX_POINTS do
    table.remove(points, 1)
  end
end

local function expire(now, fade)
  -- ★ 最后那一点（头部）永不过期。
  --
  -- 鼠标停下后就没有 on_move 了，若把点全清掉，头部那颗圆也会跟着消失——用户看到
  -- 的就是"尾迹褪尽后鼠标处空空如也"。原版靠它不会出现这种观感问题，因为原版的头部
  -- 圆帽是带体的一部分、带体没了它也没了；但设计要求是"头部总有一颗圆"。
  -- 因此这里永远留下 1 个点：尾迹照样按「尾迹长度」褪去，鼠标处始终留一颗圆。
  while #points > 1 and (now - points[1].t) >= fade do
    table.remove(points, 1)
  end
end

function on_move(x, y, t, params)
  elapsed = t
  add_point(x, y, t)
  expire(t, params.fade_duration or 1.0)
end

function step(dt, t, params)
  elapsed = t
  -- ★ 老化必须每帧都做，不能只在 on_move 里做。
  -- 原版把清理挂在 on_move 上，可一旦鼠标停下就没有 on_move 了：整条飘带会永远冻在
  -- 屏幕上不消失。step 每帧都跑，尾迹才会按「尾迹长度」逐段褪去。
  expire(t, params.fade_duration or 1.0)
end

-- 3 点滑动平均（首尾不动）。
local function smooth3(pts)
  if #pts < 3 then return end
  local tmp = {}
  for i = 1, #pts - 2 do
    tmp[i] = {
      (pts[i].x + pts[i + 1].x + pts[i + 2].x) / 3.0,
      (pts[i].y + pts[i + 1].y + pts[i + 2].y) / 3.0,
    }
  end
  for i = 1, #tmp do
    pts[i + 1].x = tmp[i][1]
    pts[i + 1].y = tmp[i][2]
  end
end

-- 构建「带体 ∪ 头部整圆」的**单条**路径；带体画不出来时返回 nil（退化为只画一颗圆）。
--
-- ★ 头部这颗圆为什么不单独画一次：它与带体在头部重叠，两次 SourceOver 会把重叠区
-- 叠成 0.6 → 0.84，头部出现一块明显的深色圆斑。原版 Python 用 `path.united(cap)`
-- 做布尔并集正是为了避开它。实现上不具备几何布尔运算，但**同一条自交多边形**在 Winding
-- 填充规则下与并集等价：圆作为一个闭合子环嵌进路径，重叠区环绕数为 2（非零）→ 只填
-- 一次，与带体同色同深浅。
--
-- 走向：尾尖 → 左侧 → L(θ=0) → 绕整圆一周回到 L → 直径 L→R → 右侧逆序 → close。
-- 带体与圆的环绕方向一致（同为顺时针），否则重叠区环绕数会抵消成 0 被挖空。
local function build_path(half_w)
  local n = #points
  if n < 3 then return nil end

  local left_raw, right_raw = {}, {}
  for i = 1, n do
    local ratio = (i - 1) / (n - 1)
    if ratio > 0.001 then -- 尾部第一个点跳过（对齐原版）
      -- 方向：端点用单侧差分，中间点用中心差分
      local dx, dy
      if i == 1 then
        dx, dy = points[2].x - points[1].x, points[2].y - points[1].y
      elseif i == n then
        dx, dy = points[n].x - points[n - 1].x, points[n].y - points[n - 1].y
      else
        dx, dy = points[i + 1].x - points[i - 1].x, points[i + 1].y - points[i - 1].y
      end
      local length = math.sqrt(dx * dx + dy * dy)
      if length >= 0.5 then
        local nx, ny = -dy / length, dx / length
        local w = half_w * ratio * ratio * ratio -- ★ 三次方收窄
        left_raw[#left_raw + 1] = { x = points[i].x + nx * w, y = points[i].y + ny * w }
        right_raw[#right_raw + 1] = { x = points[i].x - nx * w, y = points[i].y - ny * w }
      end
    end
  end

  if #left_raw < 2 then return nil end
  smooth3(left_raw)
  smooth3(right_raw)

  -- 头部处的前进方向 f 与左法线 n。左侧最后一个点正是 head + half_w·n（i==n 用后向
  -- 差分、smooth3 又不动端点），所以圆从 θ=0 起笔正好接在它上面，不会错位。
  local head, prev = points[n], points[n - 1]
  local dxh, dyh = head.x - prev.x, head.y - prev.y
  local hl = math.sqrt(dxh * dxh + dyh * dyh)
  if hl <= 0.001 then return nil end
  local fx, fy = dxh / hl, dyh / hl
  local nx, ny = -fy, fx

  local path = {}
  path[#path + 1] = { points[1].x, points[1].y }
  for i = 1, #left_raw do
    path[#path + 1] = { left_raw[i].x, left_raw[i].y }
  end
  -- 整圆：p(θ) = head + half_w · (cos θ · n + sin θ · f)，θ 从 0 绕回 0
  for k = 1, HEAD_SEGMENTS - 1 do
    local th = TAU * (k / HEAD_SEGMENTS)
    local c, s = math.cos(th), math.sin(th)
    path[#path + 1] = {
      head.x + half_w * (c * nx + s * fx),
      head.y + half_w * (c * ny + s * fy),
    }
  end
  path[#path + 1] = { head.x + half_w * nx, head.y + half_w * ny } -- 回到 L（θ=0）
  path[#path + 1] = { head.x - half_w * nx, head.y - half_w * ny } -- 直径到 R（θ=π）
  for i = #right_raw, 1, -1 do
    path[#path + 1] = { right_raw[i].x, right_raw[i].y }
  end
  return path
end

function draw(ctx, params)
  local n = #points
  if n < 1 then return end

  local half_w = (params.max_width or 20) * 0.5
  local head = points[n]

  -- 整条带 + 头部圆同色
  -- 彩虹色勾选（bool）。兼容旧配置：曾用 color_mode choice（"rainbow"/"fixed"）。
  local rb = params.rainbow
  if rb == nil then rb = (params.color_mode ~= "fixed") end
  local cm = rb and "rainbow" or "fixed"
  local r, g, b, ca
  if cm == "rainbow" then
    local hue = (elapsed * 0.3) % 1.0
    r, g, b = hsv_to_rgb(hue * 360, 1.0, 1.0)
    ca = 255
  else
    r, g, b, ca = hex_to_rgb(params.fixed_color or "#FF0000")
  end
  local a = math.floor((params.opacity or 0.5) * 255 * ca / 255)

  local path = build_path(half_w)
  if path then
    ctx:polygon(path, r, g, b, a)
  else
    -- 带体还没成形（刚起步）或已褪尽（鼠标停住）：头部那颗完整的圆仍然要画。
    ctx:circle(head.x, head.y, half_w, r, g, b, a)
  end
end
