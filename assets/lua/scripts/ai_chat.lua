-- 用法：
--   1. 在面板「脚本」页展开本卡，填 API Key（必填）、API 地址 / 模型（默认智谱 GLM，
--      任何 OpenAI /chat/completions 兼容接口都可用）。
--   2. 在「快捷键」一栏填组合键（如 Ctrl+Alt+H；留空 = 不注册热键），保存后即生效。
--   3. 按热键（或脚本页「运行」）呼出对话窗：输入框里 Ctrl+Enter 发送、Esc 关窗；
--      等待回复期间标题栏读秒，界面全程不卡。多轮上下文在窗口内保留，
--      「清除」一键清空，关窗即弃。
--
-- 辅助写脚本：系统提示词里带了一份 CurSeen Lua API 速查（./ai_chat_api.txt），
-- 直接说"帮我写一个xxx的拖尾脚本"即可，回答里给出完整 .lua 内容，粘进
-- assets/lua/MouseTrail/ 等目录保存就能热加载生效。
--
-- 交互主体在原生窗口（ext:chat_window，HTTP 在后台线程），本脚本只负责组系统
-- 提示词；详见 assets/lua/API.md。

name = "AI 写脚本"
-- ★ 刻意不写 `description`：下面四个参数（快捷键 / API 地址 / 模型 / API Key）已经说清
--   这块是干什么的，脚本页再挂一句说明就是重复。说明文字只留给**没有任何选项可设定**
--   的脚本（image role 的「图片信息」/「平均颜色」）。
-- `scripts/` 目录下 role 可省略：归属由目录决定，缺省即 composite（脚本页可运行卡片）。

function schema()
  return {
    { key = "hotkey",   label = "快捷键",   type = "hotkey", default = "" },
    { key = "ai_url",   label = "API 地址", type = "str",
      default = "https://open.bigmodel.cn/api/paas/v4/chat/completions" },
    { key = "ai_model", label = "模型",     type = "str", default = "glm-4-flash" },
    { key = "ai_api",   label = "API Key",  type = "str", default = "" },
  }
end

function run(ctx, params)
  local key = params.ai_api or ""
  if key == "" then
    ext:result("还没有 API Key。\n\n请在面板「脚本」页展开「AI 写脚本」卡片，把服务商给的 key 填进「API Key」一栏。")
    return
  end
  -- ★ 面板里把某栏**删空**保存后 params 拿到的是空串而非 nil，Lua 的 `or` 兜不住
  -- 空串（2026-09-29 实测：地址被清空 → ext:chat_window 报 "http: empty string"，
  -- 看着像 key 出了问题）。空串一律兜回出厂默认。
  local url = params.ai_url or ""
  if url == "" then url = "https://open.bigmodel.cn/api/paas/v4/chat/completions" end
  local model = params.ai_model or ""
  if model == "" then model = "glm-4-flash" end

  -- 系统提示词 = pecia 式三段（##### 分隔）：规则 / 参考资料 / 收尾重申。分段让
  -- 模型把「规则」与「资料」区别对待；语言偏好独立成句；收尾重申把最易忘的两条
  --（中文回答、不臆造 API）钉在离生成点最近的位置。速查文件每次发送都现读
  --（同目录 ./ai_chat_api.txt），改文档无需重启。
  local api = ctx.fs and ctx.fs:read("./ai_chat_api.txt")
  local api_block = api
    or "（速查文件缺失——请如实告知用户重新安装，不要臆造任何 API。）"
  local sys = table.concat({
    "##### 角色与基本规则",
    "你是 CurSeen（一款 Windows 截图 / 标注 / 录制工具）的 Lua 脚本助手。",
    "回复语言：始终使用简体中文。",
    "用户要脚本时，给出完整可直接保存为 .lua 的内容（含 role / 回调），"
      .. "并说明应放进 assets/lua 的哪个目录。",
    "",
    "##### 参考资料：CurSeen Lua API 速查",
    api_block,
    "",
    "##### 结束前重申",
    "1) 始终用简体中文回答；2) API 以速查为准，不要臆造不存在的函数。",
    "##### 以上是主要提示词，提示词结束 #####",
    "以下是与用户的对话：",
  }, "\n")

  ext:chat_window(url, model, key, sys)
end
