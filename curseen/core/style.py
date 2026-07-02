"""统一字体与样式规范 — 浅色主题

颜色：
  标题栏    #FAFAFA
  导航栏    #F5F5F5
  面板      #F0F0F0
  正文      #000000
  输入框背景 #FFFFFF
  边框      #CCCCCC
  蓝色链接  #4a9eff
"""

from PySide6.QtGui import QFont

def _font(size: int, bold: bool = False) -> QFont:
    f = QFont()
    f.setPointSize(size)
    f.setBold(bold)
    return f

H1_FONT = _font(22, bold=True)

_FONT_SIZE_PX = 16
H1_SIZE = "36px"

# ── 颜色 ──
C_WHITE    = "#000000"   # 正文黑色
C_LINK     = "#4a9eff"
C_BG       = "#F0F0F0"   # 面板背景
C_NAV_BG   = "#F5F5F5"   # 导航栏背景
C_NAV_TEXT = "#000000"
C_INPUT_BG = "#FFFFFF"
C_BORDER   = "#CCCCCC"

def get_font_size() -> int:
    return _FONT_SIZE_PX

def set_font_size(px: int):
    global _FONT_SIZE_PX
    global PANEL_BG
    global STYLE_BODY, STYLE_H1, STYLE_LINK, STYLE_MUTED, STYLE_SEP
    global STYLE_BODY_S
    global STYLE_CHECKBOX, STYLE_BTN, STYLE_COMBO, STYLE_INPUT
    global STYLE_COMBO_BLUE, STYLE_INPUT_BLUE
    global STYLE_SLIDER, STYLE_GROUPBOX, STYLE_NAV_BG, STYLE_NAV_LIST
    global STYLE_SECTION
    global LABEL_STYLE, VALUE_STYLE
    global BODY_STYLE, TITLE_STYLE, MUTED_STYLE, SEP_STYLE, LINK_STYLE
    global CHECKBOX_STYLE, BTN_STYLE, COMBO_STYLE, INPUT_STYLE
    global SLIDER_STYLE, GROUPBOX_STYLE, NAV_BG_STYLE, NAV_LIST_STYLE

    _FONT_SIZE_PX = px
    _FONT_SIZE_S_PX = max(12, px - 2)
    FS = f"{px}px"
    FS_S = f"{_FONT_SIZE_S_PX}px"
    BLACK = "#000000"
    BLUE  = "#4a9eff"

    PANEL_BG = f"background-color: {C_BG}; color: {BLACK};"

    STYLE_BODY  = f"font-size: {FS}; color: {BLACK};"
    STYLE_BODY_S = f"font-size: {FS_S}; color: {BLACK};"
    STYLE_H1    = f"font-size: {H1_SIZE}; font-weight: bold; color: {BLACK};"
    STYLE_LINK  = f"font-size: {FS}; color: {BLUE};"
    STYLE_MUTED = f"font-size: {FS}; color: {BLACK};"
    STYLE_SECTION = f"font-size: {FS}; color: {BLUE}; font-weight: bold;"
    STYLE_SEP   = "background-color: #DDD; height: 1px;"

    STYLE_CHECKBOX = f"font-size: {FS}; color: {BLACK}; spacing: 10px; " \
        "QCheckBox::indicator { width: 42px; height: 22px; border-radius: 11px; " \
        "background: #CCC; border: none; }" \
        "QCheckBox::indicator:checked { background: #4a9eff; }" \
        "QCheckBox::indicator:unchecked { background: #BBB; }"

    STYLE_BTN = f"""
        QPushButton {{
            font-size: {FS}; color: {BLACK};
            background: #E8E8E8; border: 1px solid {C_BORDER};
            padding: 6px 16px; border-radius: 4px;
        }}
        QPushButton:hover {{ background: #DDD; }}
        QPushButton:checked {{ background: #4a9eff; color: white; border-color: #4a9eff; }}
    """

    STYLE_COMBO = f"""
        QComboBox {{ font-size: {FS_S}; color: {BLACK}; background: {C_INPUT_BG}; border: 1px solid {C_BORDER}; padding: 4px 8px; border-radius: 4px; }}
        QComboBox::drop-down {{ border: none; }}
        QComboBox QAbstractItemView {{ background: {C_INPUT_BG}; color: {BLACK}; selection-background-color: #4a9eff; selection-color: white; }}
        QComboBox QAbstractItemView::item {{ min-width: 130px; padding: 4px 8px; }}
    """

    STYLE_COMBO_BLUE = f"""
        QComboBox {{ font-size: {FS_S}; color: white; background: {BLUE}; border: 1px solid {BLUE}; padding: 4px 8px; border-radius: 4px; }}
        QComboBox::drop-down {{ border: none; }}
        QComboBox QAbstractItemView {{ background: white; color: #000; selection-background-color: {BLUE}; selection-color: white; }}
        QComboBox QAbstractItemView::item {{ min-width: 130px; padding: 4px 8px; }}
    """

    STYLE_INPUT = f"font-size: {FS_S}; color: {BLACK}; background: {C_INPUT_BG}; border: 1px solid {C_BORDER}; padding: 4px 8px; border-radius: 4px;"
    STYLE_INPUT_BLUE = f"""font-size: {FS_S}; color: white; background: {BLUE}; border: 1px solid {BLUE}; padding: 4px 8px; border-radius: 4px;"""
    STYLE_PLACEHOLDER = f"color: rgba(255,255,255,180);"

    STYLE_SLIDER = f"""
        QSlider::groove:horizontal {{ height: 4px; background: #CCC; border-radius: 2px; }}
        QSlider::handle:horizontal {{ background: #4a9eff; width: 14px; height: 14px; margin: -5px 0; border-radius: 7px; }}
    """

    STYLE_GROUPBOX = f"""
        QGroupBox {{ color: {BLACK}; font-weight: bold; font-size: {FS}; border: 1px solid {C_BORDER}; border-radius: 6px;
                    margin-top: 12px; padding-top: 16px; }}
        QGroupBox::title {{ subcontrol-origin: margin; left: 12px; padding: 0 4px; }}
    """

    STYLE_NAV_BG = f"""
        QWidget {{ background-color: {C_NAV_BG}; }}
    """

    STYLE_NAV_LIST = f"""
        QListWidget {{
            background-color: {C_NAV_BG};
            border: none;
            font-size: {FS};
            color: {BLACK};
            outline: none;
        }}
        QListWidget::item {{
            padding: 10px 16px;
            border-left: 3px solid transparent;
        }}
        QListWidget::item:selected {{
            background-color: #DDD;
            color: {BLACK};
            border-left: 3px solid {BLUE};
        }}
        QListWidget::item:hover:!selected {{
            background-color: #E8E8E8;
        }}
    """

    # ── 别名 ──
    BODY_STYLE = STYLE_BODY
    TITLE_STYLE = STYLE_H1
    MUTED_STYLE = STYLE_MUTED
    SEP_STYLE = STYLE_SEP
    LINK_STYLE = STYLE_LINK
    CHECKBOX_STYLE = STYLE_CHECKBOX
    BTN_STYLE = STYLE_BTN
    COMBO_STYLE = STYLE_COMBO
    INPUT_STYLE = STYLE_INPUT
    SLIDER_STYLE = STYLE_SLIDER
    GROUPBOX_STYLE = STYLE_GROUPBOX
    LABEL_STYLE = f"font-weight: bold; font-size: {FS}; color: {BLACK};"
    VALUE_STYLE = f"font-size: {FS}; color: {BLACK};"
    NAV_BG_STYLE = STYLE_NAV_BG
    NAV_LIST_STYLE = STYLE_NAV_LIST

set_font_size(_FONT_SIZE_PX)
