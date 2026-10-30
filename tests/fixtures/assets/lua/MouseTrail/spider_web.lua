-- 蛛丝缠网：精确复刻原版 Rust spider_web.rs。锚点随机撒满**整屏**（数量 =
-- max(10, density×5)），光标只与 250px 内的锚点连线，透明度 = 不透明度 × 距离
-- 衰减 × 寿命衰减；彩虹模式整体色相随时间旋转。

effect_name = "蛛丝缠网"
name = "蛛丝缠网"
role = "effect"
function schema()
  return {
    { key = "opacity",       label = "不透明度", type = "float",  min = 0.05, max = 1,  step = 0.01, default = 1.0 },
    { key = "particle_size", label = "丝线粗细", type = "int",    min = 1,    max = 20, step = 1,    default = 10 },
    { key = "density",       label = "锚点数量", type = "int",    min = 1,    max = 50, step = 1,    default = 25 },
    { key = "fade_duration", label = "蛛丝寿命", type = "float",  min = 0.2,  max = 3,  step = 0.1,  default = 1.0 },
    { key = "rainbow",       label = "彩虹色",   type = "bool",  default = true },
    { key = "fixed_color",   label = "固定色",   type = "color",  default = "#FF0000" },
  }
end

local CONNECT_DISTANCE = 250.0

anchors = {}
elapsed = 0
cx, cy = 0, 0
has_cursor = false
anchor_count = 10

function init()
  anchors = {}
  elapsed = 0
  has_cursor = false
  anchor_count = 10
end

local function create_anchor(params)
  local fade = params.fade_duration or 1.0
  local max_width = params.particle_size or 10
  anchors[#anchors + 1] = {
    x = rng(0, screen_w),
    y = rng(0, screen_h),
    width = rng(1.0, math.max(1.0, max_width)),
    born = elapsed,
    life = fade * rng(0.8, 1.2),
  }
end

function on_move(x, y, t, params)
  elapsed = t
  cx, cy = x, y
  has_cursor = true
  -- 蛛丝只更新光标位置，不生成粒子（对齐原版）
end

function step(dt, t, params)
  elapsed = t
  -- 过期清理（swap_remove 语义 ≈ 无序删除）
  local keep = {}
  for i = 1, #anchors do
    if (elapsed - anchors[i].born) < anchors[i].life then
      keep[#keep + 1] = anchors[i]
    end
  end
  anchors = keep
  -- 补足锚点
  anchor_count = math.max(10, math.floor(params.density or 25) * 5)
  while #anchors < anchor_count do
    create_anchor(params)
  end
end

function draw(ctx, params)
  if not has_cursor or #anchors == 0 then return end
  -- 彩虹色勾选（bool）。兼容旧配置：曾用 color_mode choice（"rainbow"/"fixed"）。
  local rb = params.rainbow
  if rb == nil then rb = (params.color_mode ~= "fixed") end
  local cm = rb and "rainbow" or "fixed"
  local opacity = params.opacity or 1.0
  local fr, fg, fb, fa = hex_to_rgb(params.fixed_color or "#FF0000")
  for i = 1, #anchors do
    local a = anchors[i]
    local dx, dy = a.x - cx, a.y - cy
    local dist = math.sqrt(dx * dx + dy * dy)
    if dist < CONNECT_DISTANCE then
      local ratio = 1.0 - dist / CONNECT_DISTANCE
      local life_ratio = math.max(0.0, 1.0 - (elapsed - a.born) / a.life)
      local alpha_f = opacity * ratio * life_ratio * fa / 255
      if alpha_f > 0 then
        local r, g, b
        if cm == "rainbow" then
          local hue = (elapsed * 0.2) % 1.0
          r, g, b = hsv_to_rgb(hue * 360, 1.0, 1.0)
        else
          r, g, b = fr, fg, fb
        end
        ctx:line(cx, cy, a.x, a.y, a.width, r, g, b, math.floor(alpha_f * 255))
      end
    end
  end
end
