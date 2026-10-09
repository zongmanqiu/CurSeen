-- 屏幕标尺：以指针为交点的全屏十字线 + 坐标纸网格 + **三级固定刻度**。
--
-- 与「激光笔」「聚光灯」同属**悬浮标**家族（鼠标跟随），定位是"工程图上的游标卡尺"：
-- 主十字线告诉你精确位置，坐标纸底纹让你能**数格子测距离**，刻度像钢尺一样从主线向
-- 两侧伸出。视觉层次由淡到实：网格 < 细分刻度 < 中刻度 < 主刻度 < 主线。
--
-- ★★ 刻度为什么是**固定**的（2026-09-25 定案，改掉了"由网格间距派生"的旧做法）：
--   旧版把刻度算成「网格间距的一半 / 五分之一」——网格 100 时小刻度落在 20px 上、
--   中刻度落在 50px 上，20 不整除 50，于是每一格里出现 20/20/10/10/20/20 这种
--   **忽宽忽窄**的排布，用户直接看着就觉得"刻度不均匀、这个开关时灵时不灵"。
--   现在刻度自成一系、与网格**解耦**，三级严格整除：
--
--       主刻度 每 100px（±11px，最长最实）
--       中刻度 每  50px（±7px）   = 主刻度 ÷ 2
--       细分刻度 每 10px（±4px）  = 主刻度 ÷ 10，由「细分刻度」开关控制
--
--   100 = 2×50 = 10×10，任意两个相邻刻度之间的间距恒为 10px，**永远均匀**；
--   而且刻度从虚拟桌面原点起算，多屏下读数一致、与网格间距怎么调都无关。
--   网格（背景坐标纸）才是可调的那个参数——它只负责"数格子"的疏密。
--
-- ★ 配色巧思（为什么默认青绿）：主线不是纯色实线而是**双层描边**——先画一条更宽的
--   深色衬线、再叠主色细线。深色桌面看主色、浅色桌面看衬线，任何壁纸上都清晰，
--   无需用户为背景调颜色。
--
-- ★ 性能：网格线用 `ctx:rect`（轴对齐整数矩形走 windui 实色快路，见 spotlight.lua
--   补丁 031 注），只光栅化 1px 条带；刻度线是 ≤22px 的短线段。全帧几百条小图元，
--   无整屏光栅，开销与聚光灯同量级（≈1ms/帧）。
--
-- ★ 不透明度在**颜色里**给：颜色参数声明 `alpha = true`，取色器带上透明度条，脚本端
--   `hex_to_rgb` 的第四个返回值就是它（0–255），再按下面各层的比例分给网格/刻度/主线。
--   独立的「不透明度」参数已删除——两套透明度并存会互相打架。
--
-- 坐标系：与 spotlight 同口径，`rect`/网格从虚拟桌面 (0,0) 铺到 (screen_w,screen_h)，
-- 刻度同样从 0 起算（虚拟桌面原点），多屏下读数一致。

effect_name = "屏幕标尺"
name = "屏幕标尺"
role = "effect"
-- 三级刻度间距（**固定**，不再由网格间距派生；三者严格整除 → 均匀）。
local MAJOR = 100 -- 主刻度
local MID = 50    -- 中刻度
local MINOR = 10  -- 细分刻度

function schema()
  return {
    { key = "grid",      label = "网格间距", type = "int",   min = 50, max = 500, step = 10, default = 100 },
    { key = "show_fine", label = "细分刻度", type = "bool",                       default = true },
    { key = "color",     label = "颜色",     type = "color", alpha = true,        default = "#00FFCCBF" },
  }
end

function draw(ctx, params)
  local r, g, b, ca = hex_to_rgb(params.color or "#00FFCCBF")
  local o = ca -- 颜色自带的透明度 = 全局总开关
  if o <= 0 then
    return
  end
  local grid = params.grid or 100
  local fine = params.show_fine
  if fine == nil then
    fine = true
  end
  local mx, my = mouse_x, mouse_y

  -- 各层不透明度（颜色的 alpha 为总开关乘子；由淡到实：网格 < 细分 < 中 < 主 < 主线）。
  local a_grid  = math.floor(o * 0.13)
  -- ★ 细分刻度要看得见：旧版 0.45 那档在浅色壁纸上几乎分辨不出，"开关有没有生效"
  --   全靠猜（用户报"细分刻度时灵时不灵"的一半原因就在它太淡）。
  local a_minor = math.floor(o * 0.55)
  local a_mid   = math.floor(o * 0.72)
  local a_major = math.floor(o * 0.88)
  local a_main  = math.floor(o * 0.92)
  local a_edge  = math.floor(o * 0.35)

  -- ① 坐标纸网格：每 grid px 一条淡线（rect 1px 条带走实色快路）。
  --    只管"数格子"的疏密，与下面那套固定刻度互不干涉。
  if grid >= 50 and a_grid > 0 then
    local k = grid
    while k < screen_w do
      ctx:rect(k, 0, 1, screen_h, r, g, b, a_grid)
      k = k + grid
    end
    k = grid
    while k < screen_h do
      ctx:rect(0, k, screen_w, 1, r, g, b, a_grid)
      k = k + grid
    end
  end

  -- ② 三级对称刻度：主 100 / 中 50 / 细分 10，全部从原点起算、等距伸出主线。
  --    横向（沿水平主线）画竖短线，纵向（沿垂直主线）画横短线。
  local k = MAJOR
  while k < screen_w do
    ctx:line(k, my - 11, k, my + 11, 1, r, g, b, a_major)
    k = k + MAJOR
  end
  k = MAJOR
  while k < screen_h do
    ctx:line(mx - 11, k, mx + 11, k, 1, r, g, b, a_major)
    k = k + MAJOR
  end
  k = MID
  while k < screen_w do
    if k % MAJOR ~= 0 then -- 落在主刻度上的位置不再重复画
      ctx:line(k, my - 7, k, my + 7, 1, r, g, b, a_mid)
    end
    k = k + MID
  end
  k = MID
  while k < screen_h do
    if k % MAJOR ~= 0 then
      ctx:line(mx - 7, k, mx + 7, k, 1, r, g, b, a_mid)
    end
    k = k + MID
  end
  if fine then
    k = MINOR
    while k < screen_w do
      if k % MID ~= 0 then
        ctx:line(k, my - 4, k, my + 4, 1, r, g, b, a_minor)
      end
      k = k + MINOR
    end
    k = MINOR
    while k < screen_h do
      if k % MID ~= 0 then
        ctx:line(mx - 4, k, mx + 4, k, 1, r, g, b, a_minor)
      end
      k = k + MINOR
    end
  end

  -- ③ 双层描边十字主线：宽深色衬线垫底 + 主色细线（任意壁纸都清晰）。
  ctx:line(0, my, screen_w, my, 3, 0, 0, 0, a_edge)
  ctx:line(mx, 0, mx, screen_h, 3, 0, 0, 0, a_edge)
  ctx:line(0, my, screen_w, my, 1, r, g, b, a_main)
  ctx:line(mx, 0, mx, screen_h, 1, r, g, b, a_main)

  -- ④ 交点圆环：精确标记指针位置（主线在指针处被环"断开"的观感，读数不糊）。
  ctx:ring(mx, my, 9, 2, r, g, b, a_main)
end
