-- 双层光标：内点硬跟随，外环弹性追赶（差速迟滞），按下时环收缩回弹。
-- 平滑采用帧率无关的指数平滑，几何与运动曲线随每帧实时计算。

effect_name = "双层光标"
name = "双层光标"
role = "effect"

function schema()
  return {
    { key = "dot_r",  label = "内点半径",   type = "int",   min = 2,  max = 100, step = 1,   default = 5 },
    { key = "ring_r", label = "外环半径",   type = "int",   min = 10, max = 400, step = 1,   default = 22 },
    { key = "ring_w", label = "环宽",       type = "int",   min = 1,  max = 50,  step = 1,   default = 2 },
    { key = "follow", label = "追赶速度",   type = "float", min = 1,  max = 30,  step = 0.5, default = 12 },
    { key = "shrink", label = "按下收缩量", type = "float", min = 0,  max = 0.9, step = 0.05, default = 0.35 },
    { key = "color",  label = "颜色",       type = "color", alpha = true, default = "#3D8BD4E6" },
  }
end

-- 状态：外环位置（滞点）与最近一次按下的时刻
local rx, ry = nil, nil
local press_t = -1e9
local now = 0

function on_press(x, y, t, params)
  press_t = t
end

function step(dt, t, params)
  now = t
  local k = params.follow or 12
  local a = 1 - math.exp(-dt * k)   -- 帧率无关：tau = 1/k 秒
  if rx == nil then
    rx, ry = cursor_x, cursor_y
  end
  rx = rx + (cursor_x - rx) * a
  ry = ry + (cursor_y - ry) * a
end

function draw(ctx, params)
  if rx == nil then
    rx, ry = cursor_x, cursor_y
  end
  local r, g, b, a = hex_to_rgb(params.color or "#3D8BD4E6")
  local dot_r = params.dot_r or 5
  local ring_r = params.ring_r or 22
  local ring_w = params.ring_w or 2
  local shrink = params.shrink or 0.35

  -- 按下反馈：0.6s 内先收缩再回弹（正弦半波，中点最深）
  local age = now - press_t
  local u = age / 0.6
  if u >= 0 and u < 1 then
    ring_r = ring_r * (1 - shrink * math.sin(math.pi * u))
  end

  -- 环的外半径必须留出环体厚度，否则细环画不出任何东西（首帧判死的教训）
  if ring_r < ring_w * 1.1 then
    ring_r = ring_w * 1.1
  end

  ctx:ring(rx, ry, ring_r, ring_w, r, g, b, a)      -- 外环：弹性滞点
  ctx:circle(cursor_x, cursor_y, dot_r, r, g, b, a) -- 内点：硬跟随指针
end
