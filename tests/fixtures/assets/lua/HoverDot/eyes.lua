-- 眼睛跟随：一双眼睛缀在光标后面，眼珠始终盯着指针。
-- 眼睛本体用帧率无关的指数平滑追赶光标（滞点），眼珠从滞点望向真实指针方向；
-- 指针跑得越远眼珠偏得越开，静止后眼睛追上来、眼珠回正居中。
-- 眼珠朝指针方向偏移，但偏移量被钳制在眼白范围内（"向量钳制"）。

effect_name = "眼睛跟随"
name = "眼睛跟随"
role = "effect"

function schema()
  return {
    { key = "size",   label = "眼睛直径", type = "int",   min = 24, max = 200, step = 1,   default = 64 },
    { key = "gap",    label = "双眼间距", type = "int",   min = 0,  max = 100, step = 1,   default = 8 },
    { key = "smooth", label = "追赶速度", type = "float", min = 1,  max = 30,  step = 0.5, default = 10 },
    { key = "white",  label = "眼白颜色", type = "color", alpha = true, default = "#FFFFFFE6" },
    { key = "iris",   label = "虹膜颜色", type = "color", alpha = true, default = "#3D8BD4FF" },
    { key = "pupil",  label = "瞳孔颜色", type = "color", alpha = true, default = "#202020FF" },
  }
end

-- 状态：眼睛中心（平滑追赶光标的滞点）
local ex, ey = nil, nil

function step(dt, t, params)
  local k = params.smooth or 10
  local a = 1 - math.exp(-dt * k)   -- 帧率无关：tau = 1/k 秒
  if ex == nil then
    ex, ey = cursor_x, cursor_y
  end
  ex = ex + (cursor_x - ex) * a
  ey = ey + (cursor_y - ey) * a
end

function draw(ctx, params)
  if ex == nil then
    ex, ey = cursor_x, cursor_y
  end
  local size = params.size or 64
  local gap = params.gap or 8
  local wr, wg, wb, wa = hex_to_rgb(params.white or "#FFFFFFE6")
  local ir, ig, ib, ia = hex_to_rgb(params.iris or "#3D8BD4FF")
  local pr, pg, pb, pa = hex_to_rgb(params.pupil or "#202020FF")

  local R = size / 2                    -- 眼白半径
  local irisR = R * 0.45                -- 虹膜半径
  local pupilR = irisR * 0.42           -- 瞳孔半径
  local border = math.max(1.5, R * 0.07) -- 眼白描边宽
  -- 虹膜中心可达半径：眼白内缘再留 4% 边距（向量钳制，眼珠永远在眼白里）
  local maxOff = R - irisR - border - R * 0.04

  -- 眼珠望向指针：从眼睛滞点指向真实指针位置；200px 内线性偏移，之后打满
  local dx, dy = cursor_x - ex, cursor_y - ey
  local dist = math.sqrt(dx * dx + dy * dy)
  local ox, oy = 0.0, 0.0
  if dist > 0.5 and maxOff > 0 then
    local s = math.min(dist / 200.0, 1.0) * maxOff / dist
    ox, oy = dx * s, dy * s
  end

  -- 两只眼水平排布（中点 = 滞点，间距 gap）
  for i = -1, 1, 2 do
    local cx = ex + i * (R + gap / 2)
    local cy = ey
    ctx:circle(cx, cy, R, wr, wg, wb, wa)                     -- 眼白
    ctx:ring(cx, cy, R, border, pr, pg, pb, pa)               -- 眼白描边
    if irisR > 0 then
      ctx:circle(cx + ox, cy + oy, irisR, ir, ig, ib, ia)     -- 虹膜
    end
    if pupilR > 0 then
      ctx:circle(cx + ox, cy + oy, pupilR, pr, pg, pb, pa)    -- 瞳孔
      -- 高光：虹膜左上角一个小白点
      ctx:circle(cx + ox - irisR * 0.34, cy + oy - irisR * 0.34,
                 pupilR * 0.5, wr, wg, wb, wa)
    end
  end
end
