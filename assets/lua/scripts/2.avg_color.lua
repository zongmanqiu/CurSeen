-- 平均颜色：采样统计贴图的平均色，输出 #RRGGBB。
-- 用法：贴图上点右键 → 图片操作 → 平均颜色。
--
-- 演示 **像素读取** 的参考脚本：
--   img:bytes()  → 一次性拿回原始 RGBA 缓冲（二进制安全字符串，长度 = w*h*4，行优先）
--   data:byte(i) → Lua 字符串下标从 1 起，取第 i 个字节
--   img:pixel(x, y) → r, g, b, a（越界全 nil）——逐点访问，适合小图/稀疏采样
-- 透明像素（alpha=0）不参与平均，避免贴图镂空处把颜色拉黑。

name = "平均颜色"
description = "统计贴图平均色并输出 #RRGGBB（贴图右键 → 图片操作）"
role = "image"
menu = "平均颜色"

function run(ctx, params)
  local img = ctx.image
  if img == nil then
    return "没有贴图上下文：请在贴图上右键 → 图片操作 → 平均颜色"
  end
  local w, h = img.w, img.h
  local data = img:bytes()
  -- 步长采样：目标约 4 万个样本，整屏大图也不会卡。stride = 每隔几行/几列取一点。
  local stride = math.max(1, math.floor(math.sqrt(w * h / 40000)))
  local n, rs, gs, bs, alpha_sum = 0, 0, 0, 0, 0
  for y = 0, h - 1, stride do
    local row = y * w * 4
    for x = 0, w - 1, stride do
      local i = row + x * 4 + 1 -- RGBA，Lua 下标从 1 起
      local a = data:byte(i + 3)
      if a > 0 then
        rs = rs + data:byte(i)
        gs = gs + data:byte(i + 1)
        bs = bs + data:byte(i + 2)
        alpha_sum = alpha_sum + a
        n = n + 1
      end
    end
  end
  if n == 0 then
    return "贴图完全透明，算不出平均色"
  end
  local r = math.floor(rs / n + 0.5)
  local g = math.floor(gs / n + 0.5)
  local b = math.floor(bs / n + 0.5)
  local avg_a = math.floor(alpha_sum / n / 255 * 100 + 0.5)
  return string.format(
    "平均色：#%02X%02X%02X\nR=%d  G=%d  B=%d\n平均不透明度：%d%%\n采样：%d 点（步长 %d）",
    r, g, b, r, g, b, avg_a, n, stride)
end
