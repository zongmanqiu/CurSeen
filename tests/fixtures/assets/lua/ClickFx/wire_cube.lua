-- 点击效果 · 线框立方
--
-- 一个 8 顶点 12 边的线框立方体，从按下点浮起、双轴自旋放大，边按远近调明暗（真透视，
-- 顶点自己在 Lua 里手算，不依赖任何 shader）。全项目唯一真正的 3D 形态。
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_click(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**，与拖尾/悬浮标/独立卡片同一契约）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除
--
effect_name = "线框立方"
name = "线框立方"
role = "effect"
local cx, cy = 0.0, 0.0
local age_ms = 0.0

-- 单位立方体顶点与 12 条边（索引从 1 起）
local V = {
  { -1, -1, -1 }, { 1, -1, -1 }, { 1, 1, -1 }, { -1, 1, -1 },
  { -1, -1,  1 }, { 1, -1,  1 }, { 1, 1,  1 }, { -1, 1,  1 },
}
local E = {
  { 1, 2 }, { 2, 3 }, { 3, 4 }, { 4, 1 },
  { 5, 6 }, { 6, 7 }, { 7, 8 }, { 8, 5 },
  { 1, 5 }, { 2, 6 }, { 3, 7 }, { 4, 8 },
}
local FOV = 300.0         -- 焦距（逻辑像素）：越小透视越猛

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#7DF9FF" },
    { key = "size",     label = "棱长",     type = "int",   min = 20,  max = 300,  step = 1,    default = 90 },
    { key = "duration", label = "存活时长", type = "int",   min = 400, max = 3000, step = 10,   default = 1100 },
    { key = "spin_x",   label = "X 轴圈数", type = "float", min = -2,  max = 2,    step = 0.05, default = 0.6 },
    { key = "spin_y",   label = "Y 轴圈数", type = "float", min = -2,  max = 2,    step = 0.05, default = 1.0 },
    { key = "width",    label = "线宽",     type = "float", min = 0.6, max = 4,    step = 0.1,  default = 1.4 },
  }
end

function on_click(x, y, t, p)
  cx, cy, age_ms = x, y, 0.0
end

function step(dt, t, p)
  age_ms = age_ms + dt * 1000.0
end

function draw(ctx, p)
  local dur = p.duration or 1100
  local prog = age_ms / dur
  if prog >= 1.0 then return end       -- 淡尽：停笔，实例随即被剔除

  local r, g, b, a = hex_to_rgb(p.color or "#7DF9FF")
  local fade = 1.0 - prog
  -- 放大：0.45 → 1.0，easeOutQuad（先弹开再稳住）
  local e = 1.0 - (1.0 - prog) * (1.0 - prog)
  local half = (p.size or 90) * 0.5 * (0.45 + 0.55 * e)

  local ax = (p.spin_x or 0.6) * 6.2831853 * prog
  local ay = (p.spin_y or 1.0) * 6.2831853 * prog
  local cax, sax = math.cos(ax), math.sin(ax)
  local cay, say = math.cos(ay), math.sin(ay)

  local sx, sy, dk = {}, {}, {}
  for i = 1, 8 do
    local x, y, z = V[i][1] * half, V[i][2] * half, V[i][3] * half
    local x1 = x * cay + z * say          -- 绕 Y
    local z1 = -x * say + z * cay
    local y1 = y * cax - z1 * sax         -- 绕 X
    local z2 = y * sax + z1 * cax
    local k = FOV / (FOV + z2)            -- 透视除法（z2 远了就变小）
    sx[i] = cx + x1 * k
    sy[i] = cy + y1 * k
    dk[i] = k
  end

  local lw = p.width or 1.4
  for i = 1, 12 do
    local e1 = E[i][1]
    local e2 = E[i][2]
    -- 深度调制：k 大于 1 = 离观察者更近（更亮），小于 1 = 更远（更暗）
    local kn = ((dk[e1] + dk[e2]) * 0.5 - 0.75) * 1.6
    if kn < 0.0 then kn = 0.0 end
    if kn > 1.0 then kn = 1.0 end
    local a8 = math.floor(a * fade * (0.30 + 0.70 * kn))
    if a8 > 0 then
      ctx:line(sx[e1], sy[e1], sx[e2], sy[e2], lw * (0.7 + 0.5 * kn), r, g, b, a8)
    end
  end

  -- 顶点小点：让 8 个角"亮"起来
  local a8 = math.floor(a * fade * 0.9)
  if a8 > 0 then
    for i = 1, 8 do
      ctx:circle(sx[i], sy[i], 1.6 * (0.7 + 0.6 * dk[i]), r, g, b, a8)
    end
  end
end
