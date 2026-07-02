"""多语言支持模块"""


class LangDict(dict):
    """语言字典，支持属性访问和 format"""

    def __getattr__(self, key):
        try:
            return self[key]
        except KeyError:
            return f"!{key}!"

    def format(self, key, **kwargs):
        return self[key].format(**kwargs)


# ── 中文 ──

ZH: LangDict = LangDict({
    # ── 应用名称 ──
    "app_name": "可寻",
    "app_name_about": "可寻 • CurSeen",

    # ── 主窗口 ──
    "window_title": "可寻",
    "nav_title": "可寻",

    # ── 导航栏 ──
    "nav_trail": "🖱️  鼠标特效",
    "nav_key": "⌨️  按键显示",
    "nav_draw": "✏️  屏幕书写",
    "nav_about": "ℹ️  关于",

    # ── 全局设置 ──
    "nav_global": "⚙️  全局设置",
    "global_title": "⚙️ 全局设置",
    "global_panel_hotkey": "快捷键",
    "global_panel_hotkey_hint": "单击以捕获快捷键",
    "global_feature_toggle": "功能开关",
    "global_toggle_trail": "鼠标特效",
    "global_toggle_key": "按键显示",
    "global_toggle_draw": "屏幕书写",
    "global_reset": "默认设置",
    "global_reset_btn": "恢复默认设置",

    # ── 关于页面 ──
    "about_desc": "键鼠痕迹呈现工具：鼠标特效 · 按键显示 · 屏幕书写",
    "about_version": "版本 {version}",
    "about_label_author": "作者",
    "about_label_email": "邮箱",
    "about_label_project": "项目地址",
    "about_label_license": "许可",
    "about_label_tools": "开发工具",
    "about_tool_sep": " + ",
    "about_fallback_emoji": "🪄",

    # ── 鼠标特效 ──
    "trail_title": "🖱️ 鼠标特效",
    "trail_cursor": "悬浮标",
    "trail_cursor_status": "状态",
    "trail_cursor_off": "关闭",
    "trail_cursor_on": "开启",
    "trail_cursor_color": "颜色",
    "trail_cursor_outer": "直径",
    "trail_cursor_inner": "圆弧宽度",
    "trail_sound": "点击声音",
    "trail_sound_volume": "按键音量",
    "trail_sound_left": "左键",
    "trail_sound_right": "右键",
    "trail_ripple": "点击波纹",
    "trail_ripple_size": "波纹大小",
    "trail_ripple_duration": "持续时间",
    "trail_effect_title": "鼠标拖尾",
    "trail_effect_select": "拖尾模式",
    "trail_effect_enabled": "启用",
    "trail_ripple_left": "左键",
    "trail_ripple_right": "右键",

    # ── 按键显示 ──
    "key_title": "⌨️ 按键显示",
    "key_enable": "启用按键显示",

    # 位置
    "key_position_heading": "显示位置",
    "key_pos_bottom_left": "左下角",
    "key_pos_bottom_right": "右下角",
    "key_pos_top_left": "左上角",
    "key_pos_top_right": "右上角",

    # 偏移
    "key_offsets_heading": "边距偏移",
    "key_top_offset": "顶部偏移",
    "key_bottom_offset": "底部偏移",
    "key_left_offset": "左侧偏移",
    "key_right_offset": "右侧偏移",

    # 外观
    "key_appearance_heading": "弹窗",
    "key_popup_width": "最大宽度",
    "key_opacity": "不透明度",
    "key_fade_delay": "淡出延迟（秒）",
    "key_font_size": "字体大小",
    "key_font_color": "非功能键颜色",
    "key_fun_color": "功能键颜色",
    "key_symbol_color": "符号颜色",
    "key_bg_color": "背景颜色",

    # 过滤
    "key_filter_heading": "显示过滤",
    "key_shortcuts_only": "仅显示快捷键",
    "key_only_modifiers": "仅显示修饰键",
    "key_show_space_unicode": "空格显示为 ␣",

    # ── 显示模式 ──
    "key_display_mode": "显示内容",
    "key_display_all": "全部",
    "key_display_shortcuts_fun": "组合键 + 功能键",
    "key_display_shortcuts": "仅组合键",
    "key_show_app_icon": "显示应用图标",
    "key_merge_interval": "弹窗间隔（秒）",

    # ── 屏幕书写 ──
    "draw_title": "✏️ 屏幕书写",
    "draw_hotkeys": "快捷键",
    "draw_pen1": "1号笔",
    "draw_pen2": "2号笔",
    "draw_pen3": "3号笔",
    "draw_pen_width": "画笔粗细",

    # ── 系统托盘 ──
    "tray_tooltip": "可寻",
    "tray_show": "显示窗口",
    "tray_quit": "退出",

    # ── 自动更新 ──
    "about_update_check": "检查更新",
    "about_update_checking": "检查中…",
    "about_update_title": "软件更新",
    "about_update_error": "检查更新失败：{error}",
    "about_update_latest": "当前已是最新版本。",
    "about_update_confirm": "发现新版本 {version}，是否下载并安装？",
    "about_update_downloading": "正在下载更新…",
})

# ── 当前激活的语言 ──
LANG: LangDict = ZH
