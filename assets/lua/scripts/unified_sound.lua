-- 统一音效：每次按键都播放同一个声音（参数区「音效」浏览按钮挑的那个）。
--
-- ★ 音源（2026-09-25 改版）：schema 声明一个 `file` 参数——面板渲染成「浏览…」按钮，
--   浏览即引用：程序目录内记便携相对路径，外部记绝对路径，不复制、无中转目录。
--
-- 统一 Lua 框架契约（2026-09-23）：`role = "keysound"` + `run(ctx, params)`。
--   ctx.key  = 键名（与击键回显同源，如 "A" / "F5" / "Space"）；
--   ctx.mods = { ctrl=, alt=, shift=, win= }；
--   ctx.repeated = 是否长按自动重复；ctx.dt = 距上次按键时长。
--
-- 返回值语义：`false` = 强制静音；
--   路径 = 播它（宿主按 绝对 → 便携相对 → Sounds 解析）；
--   `nil`  = 落回出厂默认音（click3）。

name = "统一音效"
role = "keysound"

function schema()
  return {
    { key = "sound",        label = "音效",       type = "file", default = "" },
    { key = "repeat_sound", label = "长按连发出声", type = "bool", default = true },
  }
end

function run(ctx, params)
  -- 长按自动重复：用户关掉「长按连发出声」就连默认音也不发（返回 false 而非 nil）。
  if ctx.repeated and params.repeat_sound == false then
    return false
  end
  -- 挑了音就播它；没挑（空串）返回 nil → 出厂默认音。
  if params.sound ~= nil and params.sound ~= "" then
    return params.sound
  end
  return nil
end
