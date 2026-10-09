-- 分类音效：键盘五类分区配音（2026-09-21 用户指定的顺序）：功能键、修饰键、符号键、
-- 数字键、字母键各一个音；Space / Enter / Tab 等其余键归入符号键一档。
--
-- ★ 音源（2026-09-25 改版）：五个 `file` 参数——面板渲染成五个「浏览…」按钮，每一类
--   按键各挑一个音频文件（浏览即引用：程序目录内记便携相对路径，外部记绝对路径，
--   不复制、无中转目录）。
--   原来的「外部音效文件夹 + 五个下拉」方案已废弃：用户音效包里几十个音只挑五个，
--   文件浏览比从整包下拉里翻页直接得多。
--
-- 任何一类没挑（params 里空 / 未配）→ 返回 nil，落回出厂默认音。
--
-- 建框架契约（2026-09-23）：`role = "keysound"` + `run(ctx, params)`。

name = "分类音效"
role = "keysound"

-- 左右区分的修饰键名（与击键回显同源：KeyCode::display_name）。
local MOD_KEYS = {
  LCtrl = true, RCtrl = true, LShift = true, RShift = true,
  LAlt = true, RAlt = true, LWin = true, RWin = true,
}

function schema()
  return {
    { key = "fn_sound",     label = "功能键音效", type = "file", default = "" },
    { key = "mod_sound",    label = "修饰键音效", type = "file", default = "" },
    { key = "symbol_sound", label = "符号键音效", type = "file", default = "" },
    { key = "digit_sound",  label = "数字键音效", type = "file", default = "" },
    { key = "letter_sound", label = "字母键音效", type = "file", default = "" },
    { key = "repeat_sound", label = "长按连发出声", type = "bool", default = true },
  }
end

-- 该键归哪一类（返回 schema 里的参数键名）。判定顺序：功能键 → 修饰键 → 数字/字母 → 符号桶。
local function bucket_of(key, mods)
  if key:sub(1, 1) == "F" and #key <= 3 and tonumber(key:sub(2)) then
    return "fn_sound"
  end
  if MOD_KEYS[key] or mods.ctrl or mods.alt or mods.shift or mods.win then
    return "mod_sound"
  end
  if #key == 1 then
    local c = key:byte(1)
    if c >= 48 and c <= 57 then
      return "digit_sound"
    end
    if (c >= 65 and c <= 90) or (c >= 97 and c <= 122) then
      return "letter_sound"
    end
  end
  return "symbol_sound"
end

function run(ctx, params)
  if ctx.repeated and params.repeat_sound == false then
    return false
  end
  local field = bucket_of(ctx.key, ctx.mods)
  local pick = params[field]
  if pick == nil or pick == "" then
    -- 这一类没配音：落回出厂默认音。
    return nil
  end
  -- 文件是浏览按钮挑的（配置直接记路径：便携相对或绝对路径），宿主按
  -- 绝对 → 便携相对 → Sounds 出厂兜底解析；找不到时宿主自己回落默认音并记日志。
  return pick
end
