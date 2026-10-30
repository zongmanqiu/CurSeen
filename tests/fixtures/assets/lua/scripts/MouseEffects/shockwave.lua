-- MouseEffects · Shockwave
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击处扩散的冲击波：内亮外淡的圆环 + 中心闪光，可一次叠 1–4 圈。
-- 上游可选屏幕畸变；CurSeen 没有全屏位移采样，这里保留纯光环版本。

name = "Shockwave"
effect_name = "Shockwave"
kind = "click"
description = "MouseEffects Shockwave：点击扩散的冲击波圆环。"

local MAX_RINGS = 24
local rings = {}

function schema()
  return {
    { key = "count",    label = "每次圈数", type = "int",   min = 1,   max = 4,   step = 1,   default = 2 },
    { key = "radius",   label = "最大半径", type = "float", min = 100, max = 400, step = 10,  default = 260 },
    { key = "speed",    label = "扩散速度", type = "float", min = 200, max = 1200, step = 20, default = 600 },
    { key = "width",    label = "环宽",     type = "float", min = 2,   max = 20,  step = 1,   default = 8 },
    { key = "duration", label = "存在时间(秒)", type = "float", min = 0.3, max = 1.5, step = 0.05, default = 0.8 },
    { key = "glow",     label = "辉光强度", type = "float", min = 0.1, max = 3,   step = 0.1, default = 1.5 },
    { key = "color",    label = "冲击波颜色", type = "color", default = "#0080FF" },
  }
end

function init()
  rings = {}
end

function on_press(x, y, t, params)
  params = params or {}
  local n = math.floor(params.count or 2)
  for i = 1, n do
    rings[#rings + 1] = {
      x = x, y = y,
      age = -(i - 1) * 0.06,
      life = params.duration or 0.8,
    }
  end
  while #rings > MAX_RINGS do table.remove(rings, 1) end
end

function step(dt, t, params)
  params = params or {}
  for i = #rings, 1, -1 do
    local r = rings[i]
    r.age = r.age + dt
    if r.age >= r.life then table.remove(rings, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local maxR = params.radius or 260
  local speed = params.speed or 600
  local width = params.width or 8
  local glow = params.glow or 1.5
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#0080FF")
  local am = ca / 255
  for i = 1, #rings do
    local r = rings[i]
    if r.age >= 0 then
      local k = math.min(1.0, r.age / r.life)
      local radius = math.min(maxR, 10 + speed * r.age)
      local a = math.floor(230 * (1 - k) * am)
      local w = math.max(1.0, width * (1 - k * 0.6))
      ctx:ring(r.x, r.y, radius, w, cr, cg, cb, a)
      ctx:ring(r.x, r.y, radius, w * 2.4, cr, cg, cb, math.floor(a * 0.18))
      if glow > 0.2 and i == #rings then
        ctx:glow(r.x, r.y, 12 + 18 * glow, cr, cg, cb, math.floor(70 * (1 - k) * math.min(1.0, glow) * am))
      end
    end
  end
end
