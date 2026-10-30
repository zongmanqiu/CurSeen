-- 点击效果 · 随机文字
--
-- 每次按下，在点击点周围随机位置冒出几条文字（从词库里随机挑），一边上浮一边淡出。
-- 词库写在「文字」那一格，用分隔符隔开（默认 |）：
--     加油|坚持|你可以|别放弃
-- 想换分隔符就改「分隔符」那一格（比如 、 或 ; 或空格）。
--
-- ★ 本目录**唯一用文字的特效**：它走 `ctx:text_centered`（锚点 = 文字几何中心），
--   居中由渲染层按真实排版度量做——模块层拿不到字体，自己算会偏半个字。
--   （`ctx:text` 给的是排版盒左上角，做标签类排版时才用它。）
--
-- 契约（与拖尾 / 悬浮标同一套，只是触发器不同）：
--   on_press(x, y, t, p)  按下时触发一次
--   step(dt, t, p)        每帧推进（★ dt 单位是**秒**）
--   draw(ctx, p)          绘制；不再产出指令即被视为消亡并被管理器剔除

effect_name = "随机文字"
name = "随机文字"
role = "effect"

local DEFAULT_WORDS =
  "加油|坚持|你可以|别放弃|向前冲|会好的|慢慢来|来得及|做自己|超厉害|不设限|放轻松|赢定了|稳住|能行|冲"

function schema()
  return {
    { key = "words",       label = "文字",       type = "str", default = DEFAULT_WORDS },
    { key = "sep",         label = "分隔符",     type = "str", default = "|" },
    { key = "rainbow",     label = "彩虹色",     type = "bool",  default = true },
    { key = "fixed_color", label = "固定色",     type = "color", alpha = true, default = "#FF0000" },
    { key = "size",        label = "字号",       type = "int", min = 10,  max = 72,  step = 1,   default = 22 },
    { key = "count",       label = "每次几个",   type = "int", min = 1,   max = 8,   step = 1,   default = 1 },
    { key = "radius",      label = "散开半径",   type = "int", min = 0,   max = 240, step = 5,   default = 60 },
    { key = "rise",        label = "上浮距离",   type = "int", min = 0,   max = 200, step = 5,   default = 36 },
    { key = "duration",    label = "显示时长(ms)", type = "int", min = 200, max = 3000, step = 100, default = 900 },
  }
end

-- 按分隔符切词库。分隔符是**普通字符**（不是正则），故先转义再进字符类；
-- 一条都切不出来（比如分隔符写错了）就整串当一条，绝不画空白。
local function split_words(s, sep)
  local out = {}
  s = tostring(s or "")
  sep = tostring((sep ~= nil and sep ~= "") and sep or "|")
  if s == "" then
    return out
  end
  local pat = "[^" .. sep:gsub("([%^%$%(%)%%%.%[%]%*%+%-%?])", "%%%1") .. "]+"
  for word in s:gmatch(pat) do
    -- ★ 别给 for 控制变量赋值：Lua 5.5 视其为 const（宿主 5.4 虽允许，也不该依赖）
    local w = word:gsub("^%s+", ""):gsub("%s+$", "")
    if w ~= "" then
      out[#out + 1] = w
    end
  end
  if #out == 0 then
    out[1] = s
  end
  return out
end

items = {}
age = 0
local cached_key = nil
local cached_words = nil

-- 词库只在"文本 / 分隔符"真的变了时才重切（每帧切一次的开销没必要）。
local function words_of(p)
  local src = p.words or DEFAULT_WORDS
  local sep = p.sep or "|"
  local key = tostring(src) .. "\n" .. tostring(sep)
  if cached_key ~= key then
    cached_key = key
    cached_words = split_words(src, sep)
  end
  return cached_words
end

function init()
  items = {}
  age = 0
end

function on_press(x, y, t, p)
  local words = words_of(p)
  local n = clamp(math.floor(p.count or 1), 1, 8)
  local rad = p.radius or 60
  for _ = 1, n do
    local ang = rng(0, math.pi * 2)
    local dist = rng(0, 1) * rad
    items[#items + 1] = {
      x = x + math.cos(ang) * dist,
      y = y + math.sin(ang) * dist,
      w = words[rng_int(1, #words)] or "",
      hue = rng(0, 360),
    }
  end
  age = 0
end

function step(dt, t, p)
  age = (age or 0) + dt * 1000.0
end

function draw(ctx, p)
  local dur = math.max(1, p.duration or 900)
  local prog = math.min((age or 0) / dur, 1.0)
  if prog >= 1.0 then
    return -- 淡出完毕：不再产出指令，实例随即被剔除
  end

  local size = p.size or 22
  local rise = p.rise or 36
  -- 彩虹色勾选（bool）。兼容旧配置：曾用 color_mode choice（"rainbow"/"fixed"）。
  local rb = p.rainbow
  if rb == nil then rb = (p.color_mode ~= "fixed") end
  local cm = rb and "rainbow" or "fixed"
  local fr, fg, fb, fa = hex_to_rgb(p.fixed_color or "#FF0000")

  -- ★ 前 20% 全亮、其后线性淡出。刻意**不做淡入**：`age == 0` 那一帧也必须产出
  --   指令（宿主按"这一帧有没有画"判活，第一帧不画 = 当场判死）；而且文字"啪"地
  --   出现比慢慢浮现更接近本效果的语感。
  local fade = prog < 0.2 and 1.0 or (1.0 - (prog - 0.2) / 0.8)
  fade = clamp(fade, 0, 1)

  for i = 1, #items do
    local it = items[i]
    local r, g, b
    local full
    if cm == "rainbow" then
      -- 每条一个随机色相，并在存活期内缓慢推移 60° —— 一簇文字颜色各异。
      r, g, b = hsv_to_rgb((it.hue + prog * 60) % 360, 1.0, 1.0)
      full = 255
    else
      r, g, b = fr, fg, fb
      full = fa
    end
    local a = math.floor(fade * full)
    if a > 0 and it.w ~= "" then
      ctx:text_centered(it.x, it.y - rise * prog, it.w, size, r, g, b, a)
    end
  end
end
