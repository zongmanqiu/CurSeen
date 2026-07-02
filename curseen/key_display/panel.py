"""按键显示 — 完整设置面板（纯数值输入，无滑块防滚轮冲突）"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QCheckBox,
    QPushButton, QSpinBox, QScrollArea, QComboBox,
    QFrame, QColorDialog,
)
from PySide6.QtCore import Qt
from PySide6.QtGui import QColor
from PySide6.QtWidgets import QSizePolicy

from curseen.core.lang import LANG
from curseen.core.style import TITLE_STYLE, CHECKBOX_STYLE, BODY_STYLE, STYLE_SECTION, COMBO_STYLE, STYLE_COMBO_BLUE
from curseen.key_display.controller import NotificationPlacement
from curseen.key_display.monitor_preview import MonitorPreview


class KeyDisplayPanel(QWidget):
    """按键显示控制面板 — 纯数值输入"""

    SPIN_STYLE = (
        "QSpinBox { background: white; color: #000; border: 1px solid #CCC; "
        "border-radius: 4px; padding: 4px 8px; font-size: 14px; }"
        "QSpinBox::up-button, QSpinBox::down-button { "
        "width: 0px; height: 0px; }"
    )

    def __init__(self, overlay):
        super().__init__()
        self.overlay = overlay
        self._controller = None

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")
        # 自定义垂直滚动条（彻底隐藏顶部/底部按钮）
        scroll.verticalScrollBar().setStyleSheet(
            "QScrollBar:vertical { background: #E8E8E8; width: 8px; border: none; border-radius: 4px; margin: 0; }"
            "QScrollBar::handle:vertical { background: #BBB; min-height: 30px; border-radius: 4px; }"
            "QScrollBar::handle:vertical:hover { background: #999; }"
            "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { "
            "  width: 0px; height: 0px; border: none; background: transparent; }"
            "QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical { background: none; }"
        )

        content = QWidget()
        self._panel_layout = QVBoxLayout(content)
        self._panel_layout.setContentsMargins(24, 24, 24, 50)
        self._panel_layout.setSpacing(6)

        self._build_ui()

        scroll.setWidget(content)
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(scroll)

    def set_controller(self, controller):
        self._controller = controller
        if not controller:
            return
        c = controller

        def _sc():
            """保存按键显示配置到文件"""
            if hasattr(self, '_app_controller') and self._app_controller and self._app_controller.config:
                self._app_controller.config.data["key_display"] = c.to_dict()
                self._app_controller.config.save()

        # 直接传递回调，避免 parent() 链式查找
        self._monitor_preview.on_placement_changed = self.on_placement_changed

        # 先注册信号绑定：修改控件 → 更新控制器 → 即时保存
        self._top_spin.valueChanged.connect(lambda v: (
            setattr(c, 'top_offset', v),
            self._monitor_preview.set_offsets(c.top_offset, c.bottom_offset, c.left_offset, c.right_offset),
            _sc()
        ))
        self._bottom_spin.valueChanged.connect(lambda v: (
            setattr(c, 'bottom_offset', v),
            self._monitor_preview.set_offsets(c.top_offset, c.bottom_offset, c.left_offset, c.right_offset),
            _sc()
        ))
        self._left_spin.valueChanged.connect(lambda v: (
            setattr(c, 'left_offset', v),
            self._monitor_preview.set_offsets(c.top_offset, c.bottom_offset, c.left_offset, c.right_offset),
            _sc()
        ))
        self._right_spin.valueChanged.connect(lambda v: (
            setattr(c, 'right_offset', v),
            self._monitor_preview.set_offsets(c.top_offset, c.bottom_offset, c.left_offset, c.right_offset),
            _sc()
        ))

        self._width_spin.valueChanged.connect(lambda v: (setattr(c, 'popup_width', v), _sc()))
        self._opacity_spin.valueChanged.connect(lambda v: (setattr(c, 'opacity', v / 100.0), _sc()))
        self._fade_spin.valueChanged.connect(lambda v: (setattr(c, 'fade_delay', v), _sc()))
        self._font_size_spin.valueChanged.connect(lambda v: (setattr(c, 'font_size', v), _sc()))
        self._merge_spin.valueChanged.connect(lambda v: (setattr(c, 'merge_interval', v), _sc()))

        self._font_color_btn.clicked.connect(lambda: self._pick_color("font"))
        self._fun_color_btn.clicked.connect(lambda: self._pick_color("fun"))
        self._symbol_color_btn.clicked.connect(lambda: self._pick_color("symbol"))
        self._bg_color_btn.clicked.connect(lambda: self._pick_color("bg"))

        # 显示模式
        mode_map = [None, "shortcuts_fun", "shortcuts"]
        self._mode_combo.currentIndexChanged.connect(
            lambda i: (setattr(c, 'display_mode', mode_map[i]) if i > 0 else None, _sc())
        )

        c.settings_changed.connect(self._sync_from_controller)

    @staticmethod
    def _make_color_swatch(color: str, text_color: str = "white") -> QPushButton:
        """颜色小方块按钮"""
        btn = QPushButton()
        btn.setFixedSize(24, 24)
        btn.setCursor(Qt.CursorShape.PointingHandCursor)
        btn.setStyleSheet(
            f"QPushButton {{ background-color: {color}; border: 1px solid #666; "
            f"border-radius: 3px; }}"
        )
        return btn

    @staticmethod
    def _make_color_label(color: str) -> QLabel:
        """颜色代码文字标签"""
        lbl = QLabel(color)
        lbl.setStyleSheet("color: #000; font-size: 14px; background: transparent;")
        return lbl

    @staticmethod
    def _color_picker_widget(swatch: QPushButton, label: QLabel) -> QWidget:
        """颜色方块 + 文字标签 → 组合控件"""
        w = QWidget()
        w.setStyleSheet("background: transparent;")
        layout = QHBoxLayout(w)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(6)
        layout.addWidget(swatch)
        layout.addWidget(label)
        layout.addStretch()
        label.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        return w

    def _pick_color(self, target: str):
        if not self._controller:
            return
        c = self._controller
        current = c.font_color if target == "font" else c.bg_color
        color = QColorDialog.getColor(current, self, "选择颜色")
        if color.isValid():
            if target == "font":
                c.font_color = color
                self._update_color_preview(self._font_color_btn, color)
            elif target == "fun":
                c.fun_color = color
                self._update_color_preview(self._fun_color_btn, color)
            elif target == "symbol":
                c.symbol_color = color
                self._update_color_preview(self._symbol_color_btn, color)
            else:
                c.bg_color = color
                self._update_color_preview(self._bg_color_btn, color)
        self._save_config()

    def _save_config(self):
        if hasattr(self, '_app_controller') and self._app_controller and self._app_controller.config:
            self._app_controller.config.data["key_display"] = self._controller.to_dict()
            self._app_controller.config.save()

    @staticmethod
    def _update_color_preview(btn: QPushButton, color: QColor):
        """更新颜色选择器的颜色（方块背景 + 文字）"""
        btn.setStyleSheet(
            f"QPushButton {{ background-color: {color.name()}; border: 1px solid #666; "
            f"border-radius: 3px; }}"
        )
        # 更新相邻标签的文字
        parent = btn.parentWidget()
        if parent:
            labels = parent.findChildren(QLabel)
            for lbl in labels:
                if lbl is not btn:
                    lbl.setText(color.name())

    def on_placement_changed(self, placement: NotificationPlacement):
        if self._controller:
            self._controller.placement = placement
        self._save_config()

    def _sync_from_controller(self):
        """同步控制器状态到 UI（block signals 防回环）"""
        c = self._controller
        if not c:
            return
        self._monitor_preview.set_placement(c.placement)
        self._monitor_preview.set_offsets(c.top_offset, c.bottom_offset, c.left_offset, c.right_offset)
        # 更新数值时阻塞信号，防止触发 valueChanged → setattr 回环
        for spin in (self._top_spin, self._bottom_spin, self._left_spin, self._right_spin,
                     self._width_spin, self._opacity_spin, self._fade_spin, self._font_size_spin,
                     self._merge_spin):
            spin.blockSignals(True)
        self._top_spin.setValue(c.top_offset)
        self._bottom_spin.setValue(c.bottom_offset)
        self._left_spin.setValue(c.left_offset)
        self._right_spin.setValue(c.right_offset)
        self._width_spin.setValue(c.popup_width)
        self._opacity_spin.setValue(int(c.opacity * 100))
        self._fade_spin.setValue(c.fade_delay)
        self._font_size_spin.setValue(c.font_size)
        self._merge_spin.setValue(c.merge_interval)
        for spin in (self._top_spin, self._bottom_spin, self._left_spin, self._right_spin,
                     self._width_spin, self._opacity_spin, self._fade_spin, self._font_size_spin,
                     self._merge_spin):
            spin.blockSignals(False)
        self._update_color_preview(self._font_color_btn, c.font_color)
        self._update_color_preview(self._fun_color_btn, c.fun_color)
        self._update_color_preview(self._symbol_color_btn, c.symbol_color)
        self._update_color_preview(self._bg_color_btn, c.bg_color)
        # 显示模式
        mode_idx = {"all": 0, "shortcuts_fun": 1, "shortcuts": 2}.get(c.display_mode, 0)
        self._mode_combo.setCurrentIndex(mode_idx)

    # ── UI 构建 ──

    @staticmethod
    def _make_section(text: str) -> QWidget:
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

    def _make_spin_row(self, label: str, widget) -> QWidget:
        """标签 + 控件，统一宽高"""
        row = QWidget()
        row.setStyleSheet("background: transparent;")
        layout = QHBoxLayout(row)
        layout.setContentsMargins(0, 2, 0, 2)
        layout.setSpacing(8)
        lbl = QLabel(label, styleSheet=BODY_STYLE)
        lbl.setMinimumWidth(120)
        layout.addWidget(lbl)
        if not isinstance(widget, QPushButton):
            widget.setStyleSheet(self.SPIN_STYLE)
        # 所有输入控件统一尺寸
        widget.setFixedSize(160, 28)
        if isinstance(widget, QComboBox):
            widget.setSizeAdjustPolicy(QComboBox.SizeAdjustPolicy.AdjustToContents)
        layout.addWidget(widget, alignment=Qt.AlignmentFlag.AlignLeft)
        layout.addStretch()
        return row

    def _build_ui(self):
        p = self._panel_layout

        # ── 位置选择 ──
        p.addWidget(self._make_section(LANG.key_position_heading))
        self._monitor_preview = MonitorPreview()
        self._monitor_preview.setFixedSize(288, 170)
        from PySide6.QtWidgets import QApplication
        screen = QApplication.primaryScreen()
        if screen:
            geo = screen.geometry()
            self._monitor_preview.set_monitor_info(geo.width(), geo.height())
        p.addWidget(self._monitor_preview, alignment=Qt.AlignmentFlag.AlignLeft)

        # ── 偏移 ──
        p.addWidget(self._make_section(LANG.key_offsets_heading))
        self._top_spin = QSpinBox()
        self._top_spin.setRange(0, 50)
        self._top_spin.setSuffix(" %")
        p.addWidget(self._make_spin_row(LANG.key_top_offset, self._top_spin))
        self._bottom_spin = QSpinBox()
        self._bottom_spin.setRange(0, 50)
        self._bottom_spin.setSuffix(" %")
        self._bottom_spin.setValue(10)
        p.addWidget(self._make_spin_row(LANG.key_bottom_offset, self._bottom_spin))
        self._left_spin = QSpinBox()
        self._left_spin.setRange(0, 50)
        self._left_spin.setSuffix(" %")
        p.addWidget(self._make_spin_row(LANG.key_left_offset, self._left_spin))
        self._right_spin = QSpinBox()
        self._right_spin.setRange(0, 50)
        self._right_spin.setSuffix(" %")
        p.addWidget(self._make_spin_row(LANG.key_right_offset, self._right_spin))

        # ── 外观 ──
        p.addWidget(self._make_section(LANG.key_appearance_heading))
        self._width_spin = QSpinBox()
        self._width_spin.setRange(5, 50)
        self._width_spin.setSuffix(" %")
        self._width_spin.setValue(15)
        p.addWidget(self._make_spin_row(LANG.key_popup_width, self._width_spin))

        self._opacity_spin = QSpinBox()
        self._opacity_spin.setRange(5, 100)
        self._opacity_spin.setSuffix(" %")
        self._opacity_spin.setValue(100)
        p.addWidget(self._make_spin_row(LANG.key_opacity, self._opacity_spin))

        self._fade_spin = QSpinBox()
        self._fade_spin.setRange(1, 30)
        self._fade_spin.setSuffix(" s")
        self._fade_spin.setValue(5)
        p.addWidget(self._make_spin_row(LANG.key_fade_delay, self._fade_spin))

        self._merge_spin = QSpinBox()
        self._merge_spin.setRange(1, 30)
        self._merge_spin.setSuffix(" s")
        self._merge_spin.setValue(2)
        p.addWidget(self._make_spin_row(LANG.key_merge_interval, self._merge_spin))

        self._font_size_spin = QSpinBox()
        self._font_size_spin.setRange(8, 72)
        self._font_size_spin.setSuffix(" pt")
        self._font_size_spin.setValue(18)
        p.addWidget(self._make_spin_row(LANG.key_font_size, self._font_size_spin))

        # 字体颜色
        self._font_color_btn = self._make_color_swatch("#ffff00", "white")
        self._font_color_label = self._make_color_label("#ffff00")
        p.addWidget(self._make_spin_row(LANG.key_font_color, self._color_picker_widget(self._font_color_btn, self._font_color_label)))

        # Fun 键颜色
        self._fun_color_btn = self._make_color_swatch("#ff0000", "white")
        self._fun_color_label = self._make_color_label("#ff0000")
        p.addWidget(self._make_spin_row(LANG.key_fun_color, self._color_picker_widget(self._fun_color_btn, self._fun_color_label)))

        # 符号颜色
        self._symbol_color_btn = self._make_color_swatch("#ffffff", "black")
        self._symbol_color_label = self._make_color_label("#ffffff")
        p.addWidget(self._make_spin_row(LANG.key_symbol_color, self._color_picker_widget(self._symbol_color_btn, self._symbol_color_label)))

        # 背景颜色
        self._bg_color_btn = self._make_color_swatch("#000000", "white")
        self._bg_color_label = self._make_color_label("#000000")
        p.addWidget(self._make_spin_row(LANG.key_bg_color, self._color_picker_widget(self._bg_color_btn, self._bg_color_label)))

        # ── 显示内容 ──
        from PySide6.QtWidgets import QStyledItemDelegate
        from PySide6.QtGui import QPalette, QColor

        class _AlwaysDownCombo(QComboBox):
            """下拉菜单始终向下弹出"""
            def showPopup(self):
                super().showPopup()
                popup = self.view().window()
                if popup:
                    pos = self.mapToGlobal(self.rect().bottomLeft())
                    popup.move(pos)

        class _ComboDelegate(QStyledItemDelegate):
            def paint(self, painter, option, index):
                opt = option.__class__(option)
                opt.palette.setColor(QPalette.ColorRole.Text, Qt.GlobalColor.black)
                opt.palette.setColor(QPalette.ColorRole.WindowText, Qt.GlobalColor.black)
                opt.palette.setColor(QPalette.ColorRole.Highlight, QColor("#4a9eff"))
                opt.palette.setColor(QPalette.ColorRole.HighlightedText, Qt.GlobalColor.white)
                super().paint(painter, opt, index)

        self._mode_combo = _AlwaysDownCombo()
        self._mode_combo.addItems([
            LANG.key_display_all,
            LANG.key_display_shortcuts_fun,
            LANG.key_display_shortcuts,
        ])
        self._mode_combo.setItemDelegate(_ComboDelegate(self._mode_combo))
        p.addWidget(self._make_spin_row(LANG.key_display_mode, self._mode_combo))
        self._mode_combo.setStyleSheet(
            "QComboBox { background: #4a9eff; color: white; border: 1px solid #4a9eff; "
            "border-radius: 4px; padding: 4px 8px; font-size: 14px; }"
            "QComboBox::drop-down { border: none; }"
        )
        p.addStretch()

    @staticmethod
    def _sep() -> QFrame:
        line = QFrame()
        line.setFrameShape(QFrame.Shape.HLine)
        line.setStyleSheet("color: #3c3c3c;")
        return line
