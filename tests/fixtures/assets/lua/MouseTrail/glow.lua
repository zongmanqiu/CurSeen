-- 辉光圆影：精确复刻原版 Rust glow.rs（轨迹类）。每个鼠标采样点落一个光斑，
-- 按 density 抽稀绘制（step = max(1, 12 - density)），光斑半径随年龄收缩、淡出，
-- 用径向渐变（中心色 → 透明）；彩虹模式整体色相随时间旋转、且逐点错开。

effect_name = "辉光圆影"
name = "辉光圆影"
role = "effect"
function schema()
  return {
    { key = "opacity",       label = "不透明度", type = "float",  min = 0.05, max = 1,  step = 0.01, default = 0.8 },
    { key = "particle_size", label = "光斑大小", type = "int",    min = 2,    max = 40, step = 1,    default = 8 },
    { key = "density",       label = "光斑密度", type = "int",    min = 1,    max = 50, step = 1,    default = 2 },
    { key = "fade_duration", label = "尾迹长度", type = "float",  min = 0.2,  max = 3,  step = 0.1,  default = 1.0 },
    { key = "rainbow",       label = "彩虹色",   type = "bool",  default = true },
    { key = "fixed_color",   label = "固定色",   type = "color",  default = "#FF0000" },
  }
end

local MAX_POINTS = 120

points = {}
elapsed = 0

function init()
  points = {}
  elapsed = 0
end

function on_move(x, y, t, params)
  elapsed = t
  points[#points + 1] = { x = x, y = y, t = t }
  if #points > MAX_POINTS then
    table.remove(points, 1)
  end
end

function step(dt, t, params)
  elapsed = t
  -- 无粒子步进，只按 fade 过期清理
  local fade = params.fade_duration or 1.0
  local keep = {}
  for i = 1, #points do
    if elapsed - points[i].t < fade then
      keep[#keep + 1] = points[i]
    end
  end
  points = keep
end

function draw(ctx, params)
  if #points == 0 then return end
  -- 彩虹色勾选（bool）。兼容旧配置：曾用 color_mode choice（"rainbow"/"fixed"）。
  local rb = params.rainbow
  if rb == nil then rb = (params.color_mode ~= "fixed") end
  local cm = rb and "rainbow" or "fixed"
  local opacity = params.opacity or 0.8
  local glow_radius = (params.particle_size or 8) * 2.0
  local fade = params.fade_duration or 1.0
  -- 抽稀步长：density 越大越密，恒 ≥1（对齐原版 step_for）
  local density = math.floor(params.density or 2)
  local step = math.max(1, 12 - density)

  local fr, fg, fb, fa = hex_to_rgb(params.fixed_color or "#FF0000")
  local idx = 0
  for i = 1, #points, step do
    local g = points[i]
    idx = idx + 1
    local age = elapsed - g.t
    local ratio = math.max(0.0, 1.0 - age / fade)
    if ratio > 0.01 then
      local r_radius = glow_radius * ratio
      local alpha_f = ratio * opacity * fa / 255
      local a = math.floor(alpha_f * 255)
      local r, gg, b
      if cm == "rainbow" then
        local hue = (elapsed * 0.3 + i * 0.05) % 1.0
        r, gg, b = hsv_to_rgb(hue * 360, 1.0, 1.0)
      else
        r, gg, b = fr, fg, fb
      end
      -- 径向渐变：中心 color → 边缘透明
      ctx:radial(g.x, g.y, r_radius, r, gg, b, a)
    end
  end
end
