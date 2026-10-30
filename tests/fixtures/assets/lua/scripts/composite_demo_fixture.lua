-- 复合脚本示例：拖尾飘带 + 点击音效 + 点击圆环
-- ============================================================
-- 这是「复合脚本」的参考实现（role = "composite"）：一个脚本同时实现
-- 多种效果——移动画拖尾、按下播声音、按下画波纹——全部走宿主开放的 API，
-- 不借用任何内置功能。把本文件当模板复制改名，就能写自己的复合特效。
--
-- 拖尾部分采用与内置「流光飘带」同款的几何：沿轨迹构建一条**填充多边形**，
-- 宽度三次方收窄（尾尖→头部最宽）、左右边界 3 点滑动平均平滑，头部以半径=
-- 半带宽的完整圆收口。整条带单色，鼠标停住后按「拖尾时长」从尾尖逐段褪去，
-- 最后剩在鼠标处的头部小点平滑淡出（与流光飘带的差别：头部圆不永驻，
-- 时长一到屏上干干净净）。
--
-- 可用回调（全部可选，按需定义）：
--   init()                              启用/热重载后调一次
--   on_move(x, y, t, params)            鼠标采样（快移自动补插值点，与内置拖尾同密度）
--   on_mouse_down(button, x, y, params) 鼠标按下；button = "left"/"right"/"middle"/"x1"/"x2"
--                                       （★ 签名里没有时间参数，取当前时刻用全局 elapsed）
--   on_key(key, mods, repeated, params) 键盘按下；mods = {ctrl=,alt=,shift=,win=}；key 与击键回显同名
--   step(dt, t, params)                 每帧步进（清理状态、推进计时；不画画）
--   draw(ctx, params)                   每帧绘制；ctx:circle / ring / polygon / rect / layer("base"/"top") …
--
-- 全局：screen_w screen_h elapsed mouse_x mouse_y rng(min,max) rng_int(min,max)
--       clamp(v,lo,hi) lerp(a,b,t) hex_to_rgb("#RRGGBB[AA]") -> r,g,b,a
--       play_sound(文件, 音量0..100)  -- 路径解析与击键音效「音效文件」行同链，
--                                       assets/Sounds 里的裸名（如 "click3"）也可以
--
-- 改本文件即时生效（热重载）；面板里关掉开关即停。
-- ============================================================

role = "composite"
name = "复合示例：拖尾+点击音"
description = "移动画飘带拖尾，点击播放音效并画一圈波纹。可作复合脚本模板。"

function schema()
  return {
    { key = "size",        label = "圆点大小",     type = "int",   min = 2, max = 30, step = 1, default = 10 },
    { key = "color",       label = "颜色",         type = "color", alpha = true, default = "#FF000040" },
    { key = "lifetime",    label = "拖尾时长(秒)", type = "float", min = 0.5, max = 5.0, step = 0.1, default = 1.0 },
    { key = "click_sound", label = "点击音效",     type = "file",  default = "" },
    { key = "ring",        label = "点击圆环",     type = "bool",  default = true },
  }
end

-- ★ 飘带几何参数（与内置 ribbon.lua 同源）
local MIN_DIST = 2.0        -- 采样最小间距（px），太密没有视觉收益
local MAX_POINTS = 120      -- 轨迹点上限，防极端长轨迹撑爆
local HEAD_SEGMENTS = 24    -- 头部整圆细分数：24 段时最大带宽下弦高误差 0.17px
local TAU = 2.0 * math.pi

local points = {}   -- { {x=, y=, t=} } 拖尾轨迹（末位 = 头部）
local rings = {}    -- { {x=, y=, t=} } 点击效果

function init()
  points = {}
  rings = {}
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

-- ★ 老化必须每帧都做（step），不能只挂在 on_move 上——否则鼠标一停整条
--   飘带冻在屏幕上永不消失。
-- ★ 与内置流光飘带（头部圆永驻）不同：这里**全点过期**——尾巴照样从尾尖
--   逐段被吃掉，最后剩在鼠标处的头部小点按剩余寿命平滑淡出，拖尾时长到了
--   屏上干干净净（复合示例要守住"时长即消失"的直觉）。
local function expire(now, life)
  while #points > 0 and (now - points[1].t) >= life do
    table.remove(points, 1)
  end
end

function on_move(x, y, t, params)
  add_point(x, y, t)
  expire(t, params.lifetime or 1.0)
end

function on_mouse_down(button, x, y, params)
  rings[#rings + 1] = { x = x, y = y, t = elapsed }
  local f = params.click_sound or ""
  if f ~= "" then
    play_sound(f, 80)
  end
end

function step(dt, t, params)
  expire(t, params.lifetime or 1.0)
  -- 点击效果：0.6 秒寿命，到期清除
  local keep = {}
  for _, r in ipairs(rings) do
    if t - r.t < 0.6 then keep[#keep + 1] = r end
  end
  rings = keep
end

-- 3 点滑动平均（首尾不动），平滑左右边界。
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

-- 构建「带体 ∪ 头部整圆」的单条路径；带体画不出来时返回 nil（退化为只画一颗圆）。
-- 头部圆不单独画：与带体重叠区两次 SourceOver 会叠出深色圆斑，把圆作为闭合子环
-- 嵌进同一条路径，Winding 填充下与并集等价，只填一次。走向：尾尖 → 左侧 →
-- 绕整圆一周 → 直径到右 → 右侧逆序 → close（带体与圆同向，重叠区环绕数不为 0）。
local function build_path(half_w)
  local n = #points
  if n < 3 then return nil end

  local left_raw, right_raw = {}, {}
  for i = 1, n do
    local ratio = (i - 1) / (n - 1)
    if ratio > 0.001 then -- 尾部第一个点跳过
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

  -- 头部处的前进方向 f 与左法线 n
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
  local r, g, b, a = hex_to_rgb(params.color or "#FF000040")
  local life = params.lifetime or 1.0
  local half_w = (params.size or 10) * 0.5
  local head = points[#points]

  -- 整条带 + 头部圆同色（SourceOver 普通混合）
  local path = build_path(half_w)
  if path then
    ctx:polygon(path, r, g, b, a)
  elseif head then
    -- 带体还没成形（刚起步）或只剩最后一点（鼠标停住、尾巴已褪尽）：
    -- 画头部那颗圆，透明度按剩余寿命平滑淡出（exp 已把 age>=life 的点删光）。
    local age = elapsed - head.t
    if age >= 0 and age < life then
      local k = 1.0 - age / life
      ctx:circle(head.x, head.y, half_w, r, g, b, math.floor(a * k))
    end
  end

  -- 点击效果：外扩圆环（★ 缺键 = 走 schema 默认 true，只有显式 false 才关）
  if params.ring ~= false then
    for _, rg in ipairs(rings) do
      local age = elapsed - rg.t
      if age >= 0 and age < 0.6 then
        local k = age / 0.6
        ctx:ring(rg.x, rg.y, 6 + 40 * k, 3 * (1 - k) + 1, r, g, b, math.floor(a * (1 - k)))
      end
    end
  end
end
