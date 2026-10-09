-- 像素尘埃：精确复刻原版 Rust pixie_dust.rs（间隔类，spawn_batch 由控制器按
-- spawn_interval_ms × density 算好传入）。粒子四散向上飘、正弦扰动、旋转菱形核心
-- + 双层加色辉光；彩虹模式色相随时间漂移。

effect_name = "像素尘埃"
name = "像素尘埃"
role = "effect"
local TAU = math.pi * 2

function schema()
  return {
    { key = "opacity",           label = "不透明度",     type = "float",  min = 0.05, max = 1,   step = 0.01, default = 0.8 },
    { key = "particle_size",     label = "粒子大小",     type = "int",    min = 1,    max = 20,  step = 1,    default = 8 },
    { key = "density",           label = "密度",         type = "int",    min = 1,    max = 50,  step = 1,    default = 1 },
    { key = "spawn_interval_ms", label = "生成间隔(ms)", type = "int",    min = 5,    max = 200, step = 1,    default = 20 },
    { key = "fade_duration",     label = "尾迹长度",     type = "float",  min = 0.2,  max = 3,   step = 0.1,  default = 1.0 },
    { key = "rainbow",           label = "彩虹色",       type = "bool",  default = true },
    { key = "fixed_color",       label = "固定色",       type = "color",  default = "#FF0000" },
    { key = "rainbow_speed",     label = "彩虹转速",     type = "float",  min = 0,    max = 2,   step = 0.05, default = 0.5 },
    { key = "spread",            label = "横向散布",     type = "float",  min = 0,    max = 2,   step = 0.05, default = 0.5 },
    { key = "drift_speed",       label = "漂移速度",     type = "float",  min = 10,   max = 200, step = 5,    default = 50 },
  }
end

parts = {}
elapsed = 0

function init()
  parts = {}
  elapsed = 0
end

function on_move(x, y, t, params)
  elapsed = t
  local ps = params.particle_size or 8
  local fade = params.fade_duration or 1.0
  local op = params.opacity or 0.8
  local spread = params.spread or 0.5
  local drift = params.drift_speed or 50
  for _ = 1, (params.spawn_batch or 1) do
    local angle = rng(0, TAU)
    local dist = rng(0, 1) * ps * 3
    parts[#parts + 1] = {
      x = x + math.cos(angle) * dist,
      y = y + math.sin(angle) * dist,
      vx = rng(-spread * drift, spread * drift),
      vy = -rng(0.5, 1.0) * drift, -- 向上飘
      life = fade * rng(0.5, 1.0),
      maxlife = fade,
      size = ps * rng(0.6, 1.4),
      hue = rng(0, 1),
      opacity = op,
      angle = rng(0, TAU),
      spin = rng(-2.0, 2.0),
      phase = rng(0, TAU),
      turb = rng(0, 10),
    }
  end
end

function step(dt, t, params)
  elapsed = t
  local keep = {}
  for i = 1, #parts do
    local p = parts[i]
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt
    -- 确定性正弦扰动（对齐原版 M5-3：替代 random.uniform(-10,10)*dt）
    p.vy = p.vy + math.sin(p.phase + elapsed * 2.7) * p.turb * dt
    p.angle = p.angle + p.spin * dt
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
  local rs = params.rainbow_speed or 0.5
  local fr, fg, fb = hex_to_rgb(params.fixed_color or "#FF0000")
  for i = 1, #parts do
    local p = parts[i]
    local la = clamp(p.life / p.maxlife, 0, 1)
    if la > 0 then
      local alpha_f = la * p.opacity
      if alpha_f > 0 then
        local sz = p.size * la
        if sz >= 0.3 then
          local r, g, b
          if cm == "rainbow" then
            local hue = (p.hue + elapsed * rs * 0.1) % 1.0
            r, g, b = hsv_to_rgb(hue * 360, 1.0, 1.0)
          else
            r, g, b = fr, fg, fb
          end
          -- 双层加色辉光 + 旋转菱形核心（alpha 烘在颜色里，比例对齐原版）
          ctx:plus_circle(p.x, p.y, sz * 2.8, r, g, b, math.floor(alpha_f * 0.12 * 255))
          ctx:plus_circle(p.x, p.y, sz * 1.4, r, g, b, math.floor(alpha_f * 0.30 * 255))
          ctx:diamond_rot(p.x, p.y, sz, p.angle, r, g, b, math.floor(alpha_f * 255))
        end
      end
    end
  end
end
