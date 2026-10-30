-- 弹簧挂件：一串挂珠沿弹簧物理串在光标下面，跑动时甩尾、静止时垂挂晃荡。
-- 两种样式：
--   圆珠 —— 内建挂珠链（默认），不依赖任何素材，启用即见效；
--   图片 —— 一串用户图片沿"弹簧-质点链"悬挂，图片本体 = 面板「图片」行导入的
--           那张（宿主经 ctx:image 贴出，缩放按原始宽高的倍数）。没导图时什么都不画。
-- 原版只支持图片且无兜底：没导图时脚本照跑但屏幕上零输出，感知上就是"坏了"
-- （2026-10-08 用户报"无法使用"，实为悬浮标图片行为空）。

effect_name = "弹簧挂件"
name = "弹簧挂件"
role = "effect"

function schema()
  return {
    { key = "count",   label = "节数",     type = "int",    min = 2,  max = 10,   step = 1,    default = 5 },
    { key = "seg",     label = "段长",     type = "int",    min = 10, max = 200,  step = 1,    default = 36 },
    { key = "stiff",   label = "弹性",     type = "float",  min = 10, max = 200,  step = 5,    default = 60 },
    { key = "gravity", label = "重力",     type = "float",  min = 0,  max = 2000, step = 50,   default = 600 },
    { key = "style",   label = "样式",     type = "choice", choices = { "beads", "image" },
      labels = { "圆珠（无需导图）", "图片（需在面板导入）" }, default = "beads" },
    { key = "size",    label = "圆珠半径", type = "float",  min = 4,  max = 40,   step = 1,    default = 12 },
    { key = "color",   label = "圆珠颜色", type = "color",  alpha = true, default = "#00C2D8" },
    { key = "scale",   label = "图片缩放", type = "float",  min = 0.1, max = 2,   step = 0.05, default = 0.5 },
    { key = "opacity", label = "透明度",   type = "float",  min = 0,  max = 1,    step = 0.01, default = 1.0 },
  }
end

-- 状态：质点链（1 号钉在光标上，其余靠弹簧牵引）
local nodes = nil

local function ensure(n)
  if nodes ~= nil and #nodes == n then
    return
  end
  nodes = {}
  for i = 1, n do
    nodes[i] = { x = cursor_x, y = cursor_y + (i - 1) * 20, vx = 0.0, vy = 0.0 }
  end
end

function step(dt, t, params)
  local n = params.count or 5
  local seg = params.seg or 36
  local stiff = params.stiff or 60
  local grav = params.gravity or 600
  ensure(n)

  -- 1 号钉在指针上
  nodes[1].x, nodes[1].y = cursor_x, cursor_y

  local damp = math.exp(-dt * 2.5)   -- 阻尼：每秒衰减到 e^-2.5 ≈ 8%
  local margin = seg * 0.25          -- 屏幕边缘反弹的留边（贴图半宽的粗略近似）
  for i = 2, n do
    local cur = nodes[i]
    local prev = nodes[i - 1]
    -- 弹簧：加速度 = 弹性 × (当前段长 - 自然段长)，方向指向上一节
    local dx, dy = prev.x - cur.x, prev.y - cur.y
    local d = math.sqrt(dx * dx + dy * dy)
    if d > 0.001 then
      local f = stiff * (d - seg) / d
      cur.vx = cur.vx + dx * f * dt
      cur.vy = cur.vy + dy * f * dt
    end
    cur.vy = cur.vy + grav * dt
    cur.vx = cur.vx * damp
    cur.vy = cur.vy * damp
    cur.x = cur.x + cur.vx * dt
    cur.y = cur.y + cur.vy * dt
    -- 屏幕边缘反弹（恢复系数 0.6）
    if cur.x < margin then
      cur.x, cur.vx = margin, -cur.vx * 0.6
    elseif cur.x > screen_w - margin then
      cur.x, cur.vx = screen_w - margin, -cur.vx * 0.6
    end
    if cur.y < margin then
      cur.y, cur.vy = margin, -cur.vy * 0.6
    elseif cur.y > screen_h - margin then
      cur.y, cur.vy = screen_h - margin, -cur.vy * 0.6
    end
  end
end

-- 尾部轻微淡出的 alpha（与样式无关）：链的方向感更清楚；逐个 floor + clamp（宿主收 u8）
local function fade_alpha(i, n, op)
  local fade = 1 - 0.35 * (i - 1) / math.max(1, n - 1)
  return math.floor(math.max(0, math.min(255, op * fade * 255 + 0.5)))
end

-- 圆珠样式：实心圆 + 同色描边环（环略大 2px，勾出珠子的轮廓感）
local function draw_beads(c, params, n, op)
  local size = params.size or 12
  local br, bg, bb, ba = hex_to_rgb(params.color or "#00C2D8")
  for i = 1, n do
    local a = math.floor(ba / 255 * fade_alpha(i, n, op) + 0.5)
    if i == 1 then
      -- 挂点只画环：视觉上是"钩住光标"，不挡指针
      c:ring(nodes[i].x, nodes[i].y, size, 2, br, bg, bb, a)
    else
      c:circle(nodes[i].x, nodes[i].y, size, br, bg, bb, a)
      c:ring(nodes[i].x, nodes[i].y, size + 2, 1.5, br, bg, bb, a)
    end
  end
end

-- 图片样式：整条链都贴面板导入的那张图（没导图时 ctx:image 静默不画）
local function draw_images(c, params, n, scale, op)
  for i = 1, n do
    c:image(nodes[i].x, nodes[i].y, scale, fade_alpha(i, n, op))
  end
end

function draw(ctx, params)
  local n = params.count or 5
  local op = params.opacity or 1.0
  ensure(n)
  if (params.style or "beads") == "image" then
    draw_images(ctx, params, n, params.scale or 0.5, op)
  else
    draw_beads(ctx, params, n, op)
  end
end
