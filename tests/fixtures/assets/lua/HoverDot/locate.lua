-- 晃动定位：指针**静止**一段时间（默认 1 秒）后，短时间内左右晃动即弹出定位环，
-- 3 秒内淡化消失。
--
-- 与「激光笔」同属**悬浮标**家族，但它是"找指针用的"：平时屏幕上什么都不画，
-- 只在你需要的时候亮一下，告诉你"指针刚才在这儿"。
--
-- ★ 2026-09-25 重做（三条用户口径）：
--   1. **不隐藏系统指针**。旧版顶层写着 `hide_cursor = true`，宿主会把系统光标换成
--      全透明位图——用户看到的是"我的箭头没了"。那条声明连同整条隐藏链路都不再碰，
--      屏幕上永远是 Windows 自己的光标。
--   2. **不画中心圆点**。旧版还画一个 `center_size` 的圆点当"假指针"，该参数与它的
--      `ctx:circle` 一并删除：定位环只负责圈出位置，指针还是系统那个。
--   3. **触发由"累计位移"改成"静止 + 左右晃"**。旧版累计位移超阈值就弹环，正常移动
--      鼠标也会误触发。现在必须**先静止 `idle_s`**（出厂 1.0 秒）把自己"武装"起来，
--      之后短时间内发生一次水平方向反转、且反转前那一方向的累计幅度 ≥ `threshold`，
--      才弹环。弹完即解除武装，要再静止一次才能触发第二次。
--
-- ★★ 时间判据一律用 `wall_ms`（真实墙钟），**不能**用 `elapsed`：
--   `elapsed` 是宿主逐帧 capped 累加出来的，指针静止期间根本不出帧、它就不推进——
--   拿它算"静止了多久"恒等于 0，静止检测直接失效。
--
-- ★ 淡出靠 `draw` 的返回值续帧：返回一个毫秒数（这里 32 ≈ 30fps）＝"多久之后再来一
--   帧"；淡出跑完就不再返回，帧门自动收敛回休眠（静止仍是 0% CPU）。没有这条，手一停
--   环就**定格**在屏上（分层窗帧间不清缓冲）——那正是旧版"环不消失"的观感。
--
-- ★ 定位环**跟着指针走**（画在当前 `mouse_x/mouse_y`）：环亮着的那几秒里指针多半还在
--   动，画在触发那一刻的坐标会让环"掉在后面"，看着像另一个东西。

effect_name = "晃动定位"
name = "晃动定位"
role = "effect"
-- 脚本级状态（两次 draw 之间保留；宿主每帧调用 draw，指针不动就不调用）
local last_x, last_y = nil, nil  -- 上一帧指针位置
local last_move_ms = 0           -- 最后一次**移动**的墙钟时刻
local dir = 0                    -- 当前摆动方向（+1 向右 / -1 向左，0 = 未定）
local span = 0                   -- 当前方向上累计的水平幅度
local armed = false              -- 是否已"静止够久"（武装态；触发后清零）
local ring_ms = -1e9             -- 定位环诞生时刻（极小值 = 当前没有环）

function schema()
  return {
    { key = "idle_s",      label = "静止时长（秒）", type = "float", min = 0.2, max = 5.0, step = 0.2, default = 1.0 },
    { key = "threshold",   label = "晃动幅度",       type = "int",   min = 5,   max = 400,  step = 5,   default = 40 },
    { key = "show_s",      label = "显示时长（秒）", type = "float", min = 0.5, max = 5.0, step = 0.2, default = 3.0 },
    { key = "ring_radius", label = "定位环半径",     type = "int",   min = 20,  max = 400,  step = 1,   default = 50 },
    { key = "ring_width",  label = "定位环粗细",     type = "int",   min = 1,   max = 20,   step = 1,   default = 5 },
    { key = "color",       label = "定位环颜色",     type = "color", alpha = true, default = "#FF0000FF" },
  }
end

function draw(ctx, params)
  local now = wall_ms or elapsed
  -- 秒 → 毫秒（面板上以秒为单位、0.2 秒一档，脚本内部一律按毫秒算）
  local idle_ms   = (params.idle_s or 1.0) * 1000
  local threshold = params.threshold or 40
  local show_ms   = (params.show_s or 3.0) * 1000

  -- ① 静止 / 摆动判定。
  --    只在"有帧"时跑——而"有帧"本身就意味着指针刚动过，所以这里的 `gap` 正好是
  --    **这次晃动之前**那段静止的时长，不需要在静止期间计时（那段时间根本没有帧）。
  if last_x == nil then
    last_x, last_y = mouse_x, mouse_y
    last_move_ms = now
  end
  local dx = mouse_x - last_x
  local dy = mouse_y - last_y
  if math.abs(dx) + math.abs(dy) > 0.5 then
    -- ★ 先算 gap 再刷新时刻：这一帧的 gap 就是刚刚那段静止的时长。
    if now - last_move_ms >= idle_ms then
      armed = true
      dir = 0
      span = 0
    end
    last_move_ms = now
    last_x, last_y = mouse_x, mouse_y
    if math.abs(dx) >= 1.0 then
      local d = dx > 0 and 1 or -1
      if d ~= dir then
        -- 方向反转 = 一次"回摆"：已静止够久、且反转前那一方向的幅度够大 → 弹环。
        if armed and span >= threshold then
          ring_ms = now
          armed = false
        end
        dir = d
        span = 0
      else
        span = span + math.abs(dx)
      end
    end
  end

  -- ② 定位环：诞生后 show_ms 内线性淡出，到点即停（不返回 = 不再请求续帧）。
  local remain = show_ms - (now - ring_ms)
  if remain <= 0 then
    return nil
  end
  -- ★ 最终不透明度 = 淡出进度 × 颜色自带 alpha（后者由取色器的透明度条给，
  --   见 schema 里 `alpha = true`；`hex_to_rgb` 的第四个返回值）。
  local r, g, b, ca = hex_to_rgb(params.color or "#FF0000FF")
  local a = math.floor(clamp(remain / show_ms, 0, 1) * ca)
  if a <= 0 then
    return nil
  end
  -- ★ 环心取**当前**指针：环亮着的时候指针继续动，环跟着走。
  ctx:ring(mouse_x, mouse_y, params.ring_radius or 50, params.ring_width or 5, r, g, b, a)
  return 32
end
