"""屏幕书写 — 控制面板（笔设置联动+快捷键存盘）"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton, QSpinBox,
    QComboBox, QColorDialog, QScrollArea, QFrame, QSizePolicy,
)
from PySide6.QtCore import Qt
from PySide6.QtGui import QColor

from curseen.core.lang import LANG
from curseen.core.style import STYLE_BODY, STYLE_SECTION
from curseen.core.global_panel import KeyCaptureLineEdit, ToggleSwitch
from curseen.drawing.controller import DrawingController
from curseen.trail.panel import AlwaysDownCombo, _ComboDel


def make_spin_row(label: str, widget, parent=None) -> QWidget:
    row = QWidget(parent)
    row.setStyleSheet("background: transparent;")
    layout = QHBoxLayout(row)
    layout.setContentsMargins(0, 2, 0, 2)
    layout.setSpacing(8)
    lbl = QLabel(label, styleSheet=STYLE_BODY)
    lbl.setMinimumWidth(120)
    layout.addWidget(lbl)
    if isinstance(widget, QSpinBox):
        widget.setStyleSheet(
            "QSpinBox { background: white; color: #000; border: 1px solid #CCC; border-radius: 4px; padding: 4px 8px; font-size: 14px; }"
            "QSpinBox::up-button, QSpinBox::down-button { width: 0px; height: 0px; }"
        )
    widget.setFixedSize(160, 28)
    layout.addWidget(widget, alignment=Qt.AlignmentFlag.AlignLeft)
    layout.addStretch()
    return row


def make_section(text: str) -> QWidget:
    row = QWidget()
    row.setStyleSheet("background: transparent;")
    layout = QHBoxLayout(row)
    layout.setContentsMargins(0, 2, 0, 2)
    layout.setSpacing(8)
    lbl = QLabel(text)
    lbl.setStyleSheet(STYLE_SECTION)
    layout.addWidget(lbl)
    layout.addStretch()
    row.setFixedHeight(32)
    return row


class DrawingPanel(QWidget):
    """屏幕书写控制面板"""

    STYLE_MAP = {"实线": "solid", "虚线": "dashed", "点线": "dotted", "点划线": "dash_dot"}

    @staticmethod
    def _style_key(text: str) -> str:
        for k in ("实线", "虚线", "点线", "点划线"):
            if text.startswith(k):
                return k
        return "实线"

    def __init__(self, overlay, controller=None):
        super().__init__()
        self.overlay = overlay
        self._controller = controller or DrawingController(overlay)
        self._app_controller = None
        self._pen_widgets = []  # [(swatch, label, width_spin, style_combo), ...]
        self._pen_hex = ["#FF000000", "#40FF0000", "#FF00FF00"]  # 同步存储 hex
        self._build_ui()
        self._wire_controller()

    @property
    def controller(self):
        return self._controller

    def _build_ui(self):
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")
        scroll.verticalScrollBar().setStyleSheet(
            "QScrollBar:vertical { background:#E8E8E8; width:8px; border:none; border-radius:4px; margin:0; }"
            "QScrollBar::handle:vertical { background:#BBB; min-height:30px; border-radius:4px; }"
            "QScrollBar::handle:vertical:hover { background:#999; }"
            "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { width:0px; height:0px; border:none; background:transparent; }"
            "QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical { background:none; }"
        )
        content = QWidget()
        content.setStyleSheet("background: transparent;")
        layout = QVBoxLayout(content)
        layout.setContentsMargins(24, 24, 24, 50)
        layout.setSpacing(6)

        # ═══ 快捷键 ═══
        layout.addWidget(make_section(LANG.draw_hotkeys))
        hotkeys = [
            ("draw_hk_screen", "屏幕标注", "Alt+G", True),
            ("draw_hk_white", "白板模式", "Alt+W", True),
            ("draw_hk_pen1", "1号笔", "1", False),
            ("draw_hk_pen2", "2号笔", "2", False),
            ("draw_hk_pen3", "3号笔", "3", False),
            ("draw_hk_eraser", "橡皮擦", "4", False),
            ("draw_hk_line", "直线", "5", False),
            ("draw_hk_arrow", "箭头", "6", False),
            ("draw_hk_rect", "矩形", "7", False),
            ("draw_hk_ellipse", "椭圆", "8", False),
            ("draw_hk_text", "文字", "9", False),
        ]
        self._hk_widgets = {}
        for key, label, default, req_mod in hotkeys:
            row = QWidget()
            row.setStyleSheet("background: transparent;")
            rl = QHBoxLayout(row)
            rl.setContentsMargins(0, 2, 0, 2)
            rl.setSpacing(8)
            lbl = QLabel(label, styleSheet=STYLE_BODY)
            lbl.setMinimumWidth(120)
            rl.addWidget(lbl)
            hk = KeyCaptureLineEdit()
            hk._require_modifier = req_mod
            hk.set_hotkey(default)
            hk.setFixedSize(160, 28)
            hk.hotkey_changed.connect(lambda hk_str, k=key: self._save_hotkey(k, hk_str))
            rl.addWidget(hk)
            rl.addStretch()
            layout.addWidget(row)
            self._hk_widgets[key] = hk

        # 固定快捷键说明
        note = QLabel("Ctrl+Z / Backspace = 撤销\nDelete = 清空\nEsc = 退出绘图")
        note.setStyleSheet("font-size: 14px; color: #000; background: transparent; padding: 2px 0;")
        note.setContentsMargins(128, 0, 0, 0)
        layout.addWidget(note)

        # ═══ 3 支笔 ═══
        pens = [
            (LANG.draw_pen1, "#FF000000"),
            (LANG.draw_pen2, "#40FF0000"),
            (LANG.draw_pen3, "#FF00FF00"),
        ]
        for i, (title, color) in enumerate(pens):
            layout.addWidget(make_section(title))
            widths = [4, 30, 4]
            styles = ["solid", "solid", "dashed"]
            layout.addLayout(self._make_pen_section(i, color, widths[i], styles[i]))

        layout.addStretch()
        scroll.setWidget(content)
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(scroll)

    def _make_pen_section(self, idx: int, default_color: str, default_width: int = 4, default_style: str = "solid"):
        layout = QVBoxLayout()
        layout.setSpacing(6)

        def make_color_swatch(color: str) -> QPushButton:
            btn = QPushButton()
            btn.setFixedSize(20, 20)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setStyleSheet(f"QPushButton {{ background-color: {color}; border: 1px solid #666; border-radius: 3px; }}")
            return btn

        def make_color_label(color: str) -> QLabel:
            lbl = QLabel(color)
            lbl.setStyleSheet("color: #000; font-size: 14px; background: transparent;")
            return lbl

        swatch = make_color_swatch(default_color)
        lbl = make_color_label(default_color)
        cr = QWidget()
        cr.setStyleSheet("background: transparent;")
        crl = QHBoxLayout(cr)
        crl.setContentsMargins(0, 2, 0, 2)
        crl.setSpacing(8)
        crl.addWidget(swatch)
        crl.addWidget(lbl)
        crl.addStretch()
        layout.addWidget(make_spin_row("颜色", cr))
        swatch.clicked.connect(lambda checked, s=swatch, l=lbl, i=idx: self._pick_pen_color(i, s, l))

        width_spin = QSpinBox()
        width_spin.setRange(1, 500)
        width_spin.setValue(default_width)
        width_spin.setSuffix(" px")
        layout.addWidget(make_spin_row("宽度", width_spin))

        style_combo = AlwaysDownCombo()
        style_combo.addItems(["实线  ————", "虚线  — — — —", "点线  - - - - - -", "点划线  -— -— -—"])
        # 按默认样式选择初始项
        display_key = next((k for k in self.STYLE_MAP if self.STYLE_MAP[k] == default_style), "实线")
        for ci in range(style_combo.count()):
            if style_combo.itemText(ci).startswith(display_key):
                style_combo.setCurrentIndex(ci)
                break
        style_combo.setItemDelegate(_ComboDel(style_combo))
        style_combo.setStyleSheet(
            "QComboBox { background:#4a9eff; color:white; border:1px solid #4a9eff; border-radius:4px; padding:4px 8px; font-size:14px; }"
            "QComboBox::drop-down { border:none; }"
        )
        layout.addWidget(make_spin_row("线条样式", style_combo))

        self._pen_widgets.append((swatch, lbl, width_spin, style_combo))
        return layout

    def _pick_pen_color(self, idx: int, swatch, lbl):
        hex_color = self._pen_hex[idx] if idx < len(self._pen_hex) else "#FF000000"
        current = QColor(hex_color)
        color = QColorDialog.getColor(current, self, "选择颜色", QColorDialog.ColorDialogOption.ShowAlphaChannel)
        if not color or not color.isValid():
            return
        h = color.name(QColor.NameFormat.HexArgb).upper()
        test = QColor(h)
        if not test.isValid():
            h = f"#{color.alpha():02X}{color.red():02X}{color.green():02X}{color.blue():02X}"
        self._pen_hex[idx] = h
        swatch.setStyleSheet(f"QPushButton {{ background-color: {h}; border:1px solid #666; border-radius:3px; }}")
        lbl.setText(h)
        self._controller.set_pen_color(idx, color)
        if self._app_controller and self._app_controller.config:
            self._save_pens()
        self._pen_hex[idx] = h
        swatch.setStyleSheet(
            f"QPushButton {{ background-color: {h}; border:1px solid #666; border-radius:3px; }}"
        )
        lbl.setText(h)
        self._controller.set_pen_color(idx, color)
        self._save_pens()
        self._app_controller.config.save() if self._app_controller and self._app_controller.config else None

    def _wire_controller(self):
        """连接笔控件 → 控制器"""
        for idx, (swatch, lbl, width_spin, style_combo) in enumerate(self._pen_widgets):
            width_spin.valueChanged.connect(lambda v, i=idx: (self._controller.set_pen_width(i, v), self._save_pens()))
            style_combo.currentTextChanged.connect(
                lambda t, i=idx: (self._controller.set_pen_style(i, self.STYLE_MAP.get(self._style_key(t), "solid")), self._save_pens())
            )

    def _save_hotkey(self, key: str, hk_str: str):
        """即时保存快捷键到 config"""
        if not self._app_controller or not self._app_controller.config:
            return
        d = self._app_controller.config.data.setdefault("drawing", {}).setdefault("hotkeys", {})
        d[key] = hk_str
        self._app_controller.config.save()

    def _save_pens(self):
        """保存 3 支笔设置到 config"""
        if not self._app_controller or not self._app_controller.config:
            return
        pens = []
        for idx, (swatch, lbl, width_spin, style_combo) in enumerate(self._pen_widgets):
            hex_color = self._pen_hex[idx] if idx < len(self._pen_hex) else lbl.text()
            pens.append({
                "color": hex_color,
                "width": width_spin.value(),
                "style": self.STYLE_MAP.get(self._style_key(style_combo.currentText()), "solid"),
            })
        self._app_controller.config.data.setdefault("drawing", {})["pens"] = pens
        self._app_controller.config.save()

    def load_settings(self, settings: dict):
        """从配置加载笔设置和快捷键"""
        data = settings.get("drawing", {})
        pens = data.get("pens", [])
        for idx, pen_data in enumerate(pens):
            if idx >= len(self._pen_widgets):
                break
            swatch, lbl, width_spin, style_combo = self._pen_widgets[idx]
            c = pen_data.get("color", "#FF000000")
            self._pen_hex[idx] = c
            swatch.setStyleSheet(
                f"QPushButton {{ background-color: {c}; border: 1px solid #666; border-radius: 3px; }}"
            )
            lbl.setText(c)
            self._controller.set_pen_color(idx, QColor(c))
            defaults_w = [4, 30, 4]
            width_spin.setValue(pen_data.get("width", defaults_w[idx]))
            internal_style = pen_data.get("style", "solid")
            display_style = next((k for k in self.STYLE_MAP if self.STYLE_MAP[k] == internal_style), "实线")
            # 找到 combo 中匹配前缀的项
            for ci in range(style_combo.count()):
                if style_combo.itemText(ci).startswith(display_style):
                    style_combo.setCurrentIndex(ci)
                    break
        # 快捷键
        hks = data.get("hotkeys", {})
        for key, default in [
            ("draw_hk_screen", "Alt+G"), ("draw_hk_white", "Alt+W"),
            ("draw_hk_pen1", "1"), ("draw_hk_pen2", "2"), ("draw_hk_pen3", "3"),
            ("draw_hk_eraser", "4"), ("draw_hk_line", "5"), ("draw_hk_arrow", "6"),
            ("draw_hk_rect", "7"), ("draw_hk_ellipse", "8"), ("draw_hk_text", "9"),
        ]:
            if key in self._hk_widgets:
                self._hk_widgets[key].set_hotkey(hks.get(key, default))
