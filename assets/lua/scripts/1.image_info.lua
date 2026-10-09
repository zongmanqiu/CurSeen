-- 图片信息：报告当前贴图的尺寸、宽高比与像素量。
-- 用法：贴图上点右键 → 图片操作 → 图片信息。返回的文本会自动弹成可复制的结果框。
--
-- 这是一份「图片操作扩展」的最小参考脚本：
--   role = "image"          → 归入贴图动作类（脚本页出现本卡）
--   menu  = "…"             → 在贴图右键「图片操作」子菜单里登记一项（文本即菜单文本）
--   run(ctx, params)        → 触发时的入口；返回字符串 → 自动弹结果框
-- ctx.image 提供：.w / .h（只读）、pixel(x,y)、bytes()、png_base64()、save(path)。
-- 面板脚本页直接「运行」时没有贴图上下文，ctx.image 为 nil。

name = "图片信息"
description = "报告贴图尺寸、宽高比与像素量（贴图右键 → 图片操作）"
role = "image"
menu = "图片信息"

-- 最大公约数（宽高比约简用）。
local function gcd(a, b)
  while b ~= 0 do
    a, b = b, a % b
  end
  return a
end

function run(ctx, params)
  local img = ctx.image
  if img == nil then
    return "没有贴图上下文：请在贴图上右键 → 图片操作 → 图片信息"
  end
  local w, h = img.w, img.h
  local g = gcd(w, h)
  if g == 0 then g = 1 end
  local orientation = "正方形"
  if w > h then
    orientation = "横图"
  elseif w < h then
    orientation = "竖图"
  end
  return string.format(
    "尺寸：%d × %d 像素\n宽高比：%d:%d（约 %.3f）\n像素量：%.2f MP\n朝向：%s",
    w, h, w // g, h // g, w / h, w * h / 1000000, orientation)
end
