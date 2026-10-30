-- 点击效果 · 多重光环
--
-- 一圈圈同心圆环递次扩散，外圈最亮、越靠内越暗。环数与描边粗细可调。
-- §39 起删除中心实心辉光（要实心点用「实心圆」），描边粗细改为显式参数。
--
-- 可用绘制原语（见 scripting/api_draw.rs）：
--   ctx:circle(x,y,r, r,g,b,a)             实心圆（SourceOver）
--   ctx:ring(x,y,outer_r,w, r,g,b,a)       圆环（outer_r 外半径，w 向内收）
--   ctx:plus_circle / glow / radial / ... 

effect_name = "多重光环"
name = "多重光环"
role = "effect"
function schema()
  return {
    { key = "color",     label = "颜色",     type = "color", alpha = true, default = "#FF0000" },
    { key = "max_size",  label = "最大半径", type = "int",   min = 20, max = 400, step = 1, default = 100 },
    { key = "duration",  label = "扩散时长", type = "int",   min = 100, max = 3000, step = 10, default = 500 },
    { key = "rings",     label = "环数",     type = "int",   min = 1, max = 4, step = 1, default = 2 },
    { key = "thickness", label = "描边粗细", type = "int",   min = 1, max = 60, step = 1, default = 5 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age = x, y, 0
end

function step(dt, t, p)
  age = (age or 0) + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 500
  local prog = math.min((age or 0) / dur, 1.0)
  if prog >= 1.0 then return end

  -- 颜色自带透明度（8 位 #RRGGBBAA，alpha 在**末尾**）；最终不透明度 = 颜色 alpha × 淡出进度，
  -- 内环再逐档调暗（外圈最亮，越靠内越暗）。
  local r, g, b, a = hex_to_rgb(p.color or "#FF0000")
  local max_r = p.max_size or 100
  local size = max_r * (0.15 + prog * 0.85)
  local w = math.max(1.0, p.thickness or 5)
  local n = p.rings or 2
  for i = 0, n - 1 do
    local rr = size * (1.0 - i * 0.28)
    -- ctx:ring 第 3 个参数是**外半径**，中心线半径要再加半个笔宽
    local outer = rr + w * 0.5
    -- ★ 半径下限（2026-09-27）：描边粗细拉到 15 以上时，首帧的 size（= 最大半径 × 0.15）
    --   可能比 w 还小，于是每个环都被下面的判断挡掉、整帧零指令——而按下事件若落在两帧
    --   之间，宿主确实会在 age 仍是 0 时画这一帧，实例当场被判死，表现为**点了没反应**。
    --   垫到 1.1×w：既保证画得出来，又避开 ctx:ring 在 `width ≥ 外半径` 时退化成实心圆。
    if outer < w * 1.1 then outer = w * 1.1 end
    ctx:ring(cx, cy, outer, w, r, g, b,
             math.floor(a * (1.0 - prog) * (1.0 - i * 0.3)))
  end
end
