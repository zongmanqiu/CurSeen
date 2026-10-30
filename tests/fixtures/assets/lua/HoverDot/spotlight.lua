-- 聚光灯：整块屏幕压暗，只在指针周围留一个透亮的光圈（跟随指针移动）。
--
-- 与「激光笔」同属**悬浮标**家族（固定形状 + 鼠标跟随），只是画的相反：激光笔在指针上
-- 落一个点，聚光灯把**除光圈以外**的整块屏幕压暗。
--
-- ★ 性能写法（为什么不再用整屏多边形）：
--   旧实现用一次 `ctx:polygon` 填一个"盖满全屏、中心挖洞"的环带多边形 —— 光栅器要对
--   几乎整屏的每个像素算覆盖率，1920×1080 下约 **11 ms/帧**，是「一开聚光灯就卡、
--   CPU 从 0 跳到 8%」的真正主因。
--   新写法拆成两步，都避开了整屏光栅：
--     1. `ctx:rect(0,0,screen_w,screen_h, ...)` 整屏压暗：轴对齐整数矩形，走 windui
--        实色快路（CurSeen 补丁 031），只按行直写 ≈ 0.5 ms，不再逐像素扫描；
--     2. `ctx:clear_circle(cx,cy,r)` 在暗罩上挖洞：Clear 混合把圆内像素清成透明，只
--        光栅化一个 ≈ 2r×2r 的小圆（r=140 时约 0.1 ms）。
--   两步叠加 ≈ 0.6 ms/帧，与旧实现视觉结果逐像素一致（中心透明、四周压暗、透亮面积 = πr²）。
--
-- 注意：暗罩不依赖屏幕原点（多屏负坐标），`rect` 从 (0,0) 铺到 (screen_w,screen_h) 即
-- 覆盖本屏全部内容；clear_circle 以指针为心，自然跟随。

effect_name = "聚光灯"
name = "聚光灯"
role = "effect"
function schema()
  return {
    { key = "radius", label = "光圈半径", type = "int", min = 10, max = 300, step = 1, default = 100 },
    { key = "dim",    label = "暗罩浓度", type = "int", min = 0,  max = 255, step = 1, default = 100 },
  }
end

function draw(ctx, params)
  -- ★ 自声明「垫底层」：暗罩用 clear_circle 挖洞是破坏性擦除，必须最先画，才会只清掉
  -- 自己的暗罩、让叠在上面的波纹/拖尾保持可见。有了这行，聚光灯无论被当作悬浮标还是
  -- 放进鼠标拖尾目录当特效，暗罩顺序都正确，不依赖被加载到哪个目录。
  ctx:layer("base")

  local cx, cy = mouse_x, mouse_y
  local r = params.radius or 100
  local a = params.dim or 100
  -- 浓度为 0 = 完全不压暗：连指令都不必发。
  if a <= 0 or r <= 0 then
    return
  end

  -- ① 整屏压暗：轴对齐矩形走 windui 实色快路（补丁 031）。
  ctx:rect(0, 0, screen_w, screen_h, 0, 0, 0, a)
  -- ② 圆形光圈：在暗罩上挖洞（Clear 混合，只光栅化小圆）。
  ctx:clear_circle(cx, cy, r)
end
