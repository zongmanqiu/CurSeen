-- 图片悬浮标：静态 PNG（**保持原始宽高**）或 GIF 动图（逐帧播放）。
-- 图片本体由面板「图片」行浏览导入并复制到本目录，配置只记文件名；
-- 位图/动画帧由宿主每帧预加载后经 ctx:image(...) 贴出。

effect_name = "图片"
name = "图片"
role = "effect"
function schema()
  return {
    { key = "scale",   label = "缩放",     type = "float", min = 0.1, max = 1, step = 0.01, default = 1.0 },
    { key = "opacity", label = "不透明度", type = "float", min = 0, max = 1,  step = 0.01, default = 0.5 },
  }
end

function draw(ctx, params)
  -- 缩放是**倍数**：宿主按图片原始宽高 × scale 贴出（宽高比不会被破坏），
  -- 并把鼠标对准图片中心。故脚本不需要知道图片多大。
  local scale = params.scale or 1.0
  local a = math.floor((params.opacity or 0.5) * 255)
  ctx:image(mouse_x, mouse_y, scale, a)
end
