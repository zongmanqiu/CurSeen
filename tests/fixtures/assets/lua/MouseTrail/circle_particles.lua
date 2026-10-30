-- 律动粒子：精确复刻原版 Rust circle_particles.rs（间隔类）。粒子从鼠标位置向
-- 四面八方飞散、受重力下坠、普通填充圆（无辉光），色相固定不随时间旋转。

effect_name = "律动粒子"
name = "律动粒子"
role = "effect"
local TAU = math.pi * 2

function schema()
  return {
    { key = "opacity",           label = "不透明度",     type = "float",  min = 0.05, max = 1,   step = 0.01, default = 0.8 },
    { key = "particle_size",     label = "粒子大小",     type = "int",    min = 1,    max = 20,  step = 1,    default = 5 },
    { key = "density",           label = "密度",         type = "int",    min = 1,    max = 50,  step = 1,    default = 5 },
    { key = "spawn_interval_ms", label = "生成间隔(ms)", type = "int",    min = 5,    max = 200, step = 1,    default = 20 },
    { key = "fade_duration",     label = "尾迹长度",     type = "float",  min = 0.2,  max = 3,   step = 0.1,  default = 1 },
    { key = "rainbow",           label = "彩虹色",       type = "bool",  default = true },
    { key = "fixed_color",       label = "固定色",       type = "color",  default = "#FF0000" },
    { key = "spread_speed",      label = "飞散速度",     type = "float",  min = 20,   max = 300, step = 5,    default = 100 },
    { key = "gravity",           label = "重力",         type = "float",  min = 0,    max = 200, step = 5,    default = 20 },
  }
end

parts = {}

function init()
  parts = {}
end

function on_move(x, y, t, params)
  local ps = params.particle_size or 5
  local fade = params.fade_duration or 1
  local op = params.opacity or 0.8
  local spread_speed = params.spread_speed or 100
  for _ = 1, (params.spawn_batch or 1) do
    local angle = rng(0, TAU)
    local speed = rng(0.3, 1.0) * spread_speed
    parts[#parts + 1] = {
      x = x + rng(-3.0, 3.0),
      y = y + rng(-3.0, 3.0),
      vx = math.cos(angle) * speed,
      vy = math.sin(angle) * speed,
      life = fade * rng(0.4, 1.0),
      maxlife = fade,
      size = ps * rng(0.5, 1.0),
      hue = rng(0, 1),
      opacity = op,
    }
  end
end

function step(dt, t, params)
  local gravity = params.gravity or 20
  local keep = {}
  for i = 1, #parts do
    local p = parts[i]
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt
    p.vy = p.vy + gravity * dt -- 重力向下为正
    p.life = p.life - dt
    if p.life > 0 then keep[#keep + 1] = p end
  end
  parts = keep
end

function draw(ctx, params)
  -- 彩虹色勾选（bool）。兼容旧配置：曾用 color_mode choice（"rainbow"/"fixed"）。
  local rb = params.rainbow
  if rb == nil then rb = (params.color_mode ~= "fixed") end
  local cm = rb and "rainbow" or "fixed"
  local fr, fg, fb, fa = hex_to_rgb(params.fixed_color or "#FF0000")
  for i = 1, #parts do
    local p = parts[i]
    local la = clamp(p.life / p.maxlife, 0, 1)
    if la > 0 then
      local alpha_f = la * p.opacity * fa / 255
      if alpha_f > 0 then
        local sz = p.size * la
        if sz >= 0.3 then
          local r, g, b
          if cm == "rainbow" then
            -- 注意：律动粒子不做色相随时间旋转（hue 固定，对齐原版）
            r, g, b = hsv_to_rgb(p.hue * 360, 1.0, 1.0)
          else
            r, g, b = fr, fg, fb
          end
          ctx:circle(p.x, p.y, sz, r, g, b, math.floor(alpha_f * 255))
        end
      end
    end
  end
end
