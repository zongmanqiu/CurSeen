"""鼠标特效设置面板"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QSpinBox, QScrollArea,
    QPushButton, QComboBox, QFrame, QColorDialog, QSizePolicy,
    QButtonGroup,
)
from PySide6.QtCore import Qt, Signal, QPropertyAnimation, QRectF
from PySide6.QtGui import QColor, QPainter, QBrush, QPen, QPalette

from curseen.core.lang import LANG
from curseen.core.style import (
    STYLE_BODY, STYLE_SECTION, CHECKBOX_STYLE, COMBO_STYLE,
)
from curseen.trail.controller import EFFECT_REGISTRY
from curseen.core.global_panel import ToggleSwitch
from PySide6.QtWidgets import QStyledItemDelegate


# ── 通用：总是向下弹出的下拉框 ──

class AlwaysDownCombo(QComboBox):
    def showPopup(self):
        super().showPopup()
        popup = self.view().window()
        if popup:
            pos = self.mapToGlobal(self.rect().bottomLeft())
            popup.move(pos)

class _ComboDel(QStyledItemDelegate):
    def paint(self, painter, option, index):
        opt = option.__class__(option)
        opt.palette.setColor(QPalette.ColorRole.Text, Qt.GlobalColor.black)
        opt.palette.setColor(QPalette.ColorRole.WindowText, Qt.GlobalColor.black)
        opt.palette.setColor(QPalette.ColorRole.Highlight, QColor("#4a9eff"))
        opt.palette.setColor(QPalette.ColorRole.HighlightedText, Qt.GlobalColor.white)
        super().paint(painter, opt, index)


# ═══════════════════════════════════════════════════════
# 5 种开关样式
# ═══════════════════════════════════════════════════════

class _ToggleBase(QWidget):
    """开关基类"""
    toggled = Signal(bool)
    stateChanged = Signal(int)

    def __init__(self, checked=True, parent=None):
        super().__init__(parent)
        self._checked = checked
        self._on_change = None   # 外部回调: fn(checked: bool)
        self.setCursor(Qt.CursorShape.PointingHandCursor)

    def isChecked(self): return self._checked

    def setChecked(self, checked):
        self._checked = checked
        self.update()

    def _emit(self):
        self.toggled.emit(self._checked)
        self.stateChanged.emit(2 if self._checked else 0)


# ── 1. 椭圆胶囊（QCheckBox 样式） ──

class CapsuleToggle(_ToggleBase):
    def __init__(self, checked=True, parent=None):
        super().__init__(checked, parent)
        self.setFixedSize(42, 24)

    def mousePressEvent(self, e):
        self._checked = not self._checked
        self.update(); self._emit()
        if self._on_change:
            self._on_change(self._checked)

    def paintEvent(self, e):
        p = QPainter(self); p.setRenderHint(QPainter.RenderHint.Antialiasing)
        w, h = self.width(), self.height()
        r = h / 2
        if self._checked:
            p.setBrush(QBrush(QColor("#4a9eff"))); p.setPen(Qt.PenStyle.NoPen)
        else:
            p.setBrush(QBrush(QColor("#E8E8E8"))); p.setPen(QPen(QColor("#CCC"), 1))
        p.drawRoundedRect(QRectF(0, 0, w, h), r, r)
        knob_x = w - h + 2 if self._checked else 2
        p.setBrush(QBrush(QColor("#FFFFFF"))); p.setPen(QPen(QColor("#DDD"), 0.5))
        p.drawEllipse(QRectF(knob_x, 2, h - 4, h - 4))
        p.end()


# ── 2. 双按钮分段 ──

class SegmentedToggle(_ToggleBase):
    def __init__(self, checked=True, parent=None):
        super().__init__(checked, parent)
        self.setFixedSize(80, 24)
        layout = QHBoxLayout(self); layout.setContentsMargins(0,0,0,0); layout.setSpacing(0)
        self._on = QPushButton("开启"); self._off = QPushButton("关闭")
        for b in (self._on, self._off):
            b.setFixedSize(40, 24); b.setCursor(Qt.CursorShape.PointingHandCursor)
            b.setFont(self.font()); b.setStyleSheet("border: 1px solid #4a9eff; font-size: 12px;")
        self._on.setStyleSheet(self._on.styleSheet() + " border-right: none; border-radius: 4px 0 0 4px;")
        self._off.setStyleSheet(self._off.styleSheet() + " border-left: none; border-radius: 0 4px 4px 0;")
        layout.addWidget(self._on); layout.addWidget(self._off)
        self._update()
        self._on.clicked.connect(lambda: self._set(True))
        self._off.clicked.connect(lambda: self._set(False))

    def _set(self, v):
        if v != self._checked:
            self._checked = v; self._update(); self._emit()

    def _update(self):
        blue = "background:#4a9eff;color:white;"
        gray = "background:#f5f5f5;color:#000;"
        on_s = self._on.styleSheet().split("background")[0] + "background" + (blue.split("background")[1] if self._checked else gray.split("background")[1])
        off_s = self._off.styleSheet().split("background")[0] + "background" + (blue.split("background")[1] if not self._checked else gray.split("background")[1])
        self._on.setStyleSheet(on_s); self._off.setStyleSheet(off_s)

    def setChecked(self, c): self._set(c)


# ── 3. iOS 滑动 ──

class IOSSlide(_ToggleBase):
    def __init__(self, checked=True, parent=None):
        super().__init__(checked, parent)
        self.setFixedSize(44, 24)
        self._knob = 1.0 if checked else 0.0
        self._anim = QPropertyAnimation(self, b"knob", self)
        self._anim.setDuration(150)

    def get_knob(self): return self._knob
    def set_knob(self, v): self._knob = v; self.update()
    knob = property(get_knob, set_knob)

    def mousePressEvent(self, e):
        self._checked = not self._checked
        t = 1.0 if self._checked else 0.0
        self._anim.setStartValue(self._knob); self._anim.setEndValue(t); self._anim.start()
        self._emit()

    def paintEvent(self, e):
        p = QPainter(self); p.setRenderHint(QPainter.RenderHint.Antialiasing)
        w, h = self.width(), self.height()
        track_h = 20; ty = (h - track_h) / 2; r = track_h / 2
        p.setBrush(QBrush(QColor("#34C759") if self._checked else QColor("#E8E8E8")))
        p.setPen(Qt.PenStyle.NoPen)
        p.drawRoundedRect(QRectF(0, ty, w, track_h), r, r)
        p.setBrush(QBrush(QColor("#FFFFFF"))); p.setPen(QPen(QColor("#DDD"), 0.5))
        kx = 2 + self._knob * (w - 24)
        p.drawEllipse(QRectF(kx, 2, 20, 20))
        p.end()


# ── 4. 方形带勾 ──

class SquareToggle(_ToggleBase):
    def __init__(self, checked=True, parent=None):
        super().__init__(checked, parent)
        self.setFixedSize(24, 24)

    def mousePressEvent(self, e):
        self._checked = not self._checked; self.update(); self._emit()

    def paintEvent(self, e):
        p = QPainter(self); p.setRenderHint(QPainter.RenderHint.Antialiasing)
        if self._checked:
            p.setBrush(QBrush(QColor("#4a9eff"))); p.setPen(Qt.PenStyle.NoPen)
            p.drawRoundedRect(QRectF(0, 0, 24, 24), 4, 4)
            p.setPen(QPen(QColor("#FFFFFF"), 3))
            p.drawLine(5, 12, 10, 18); p.drawLine(10, 18, 19, 6)
        else:
            p.setBrush(QBrush(QColor("#F5F5F5"))); p.setPen(QPen(QColor("#CCC"), 1))
            p.drawRoundedRect(QRectF(0, 0, 24, 24), 4, 4)
        p.end()


# ── 5. 圆形填充 ──

class CircleToggle(_ToggleBase):
    def __init__(self, checked=True, parent=None):
        super().__init__(checked, parent)
        self.setFixedSize(24, 24)

    def mousePressEvent(self, e):
        self._checked = not self._checked; self.update(); self._emit()

    def paintEvent(self, e):
        p = QPainter(self); p.setRenderHint(QPainter.RenderHint.Antialiasing)
        cx, cy, r = 12, 12, 10
        if self._checked:
            p.setBrush(QBrush(QColor("#4a9eff"))); p.setPen(Qt.PenStyle.NoPen)
            p.drawEllipse(QRectF(cx - r, cy - r, 2 * r, 2 * r))
            p.setPen(QPen(QColor("#FFFFFF"), 2.5))
            p.drawLine(cx - 4, cy, cx - 1, cy + 3); p.drawLine(cx - 1, cy + 3, cx + 4, cy - 2)
        else:
            p.setBrush(QBrush(QColor("#F5F5F5"))); p.setPen(QPen(QColor("#CCC"), 2))
            p.drawEllipse(QRectF(cx - r, cy - r, 2 * r, 2 * r))
        p.end()


# ── 公共函数 ──

def make_color_swatch(color: str) -> QPushButton:
    btn = QPushButton()
    btn.setFixedSize(24, 24)
    btn.setCursor(Qt.CursorShape.PointingHandCursor)
    btn.setStyleSheet(f"QPushButton {{ background-color: {color}; border: 1px solid #666; border-radius: 3px; }}")
    return btn


def make_color_label(color: str) -> QLabel:
    lbl = QLabel(color)
    lbl.setStyleSheet("color: #000; font-size: 14px; background: transparent;")
    return lbl


def color_picker_widget(swatch, label) -> QWidget:
    w = QWidget(); w.setStyleSheet("background: transparent;")
    layout = QHBoxLayout(w); layout.setContentsMargins(0,0,0,0); layout.setSpacing(6)
    layout.addWidget(swatch); layout.addWidget(label); layout.addStretch()
    label.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
    return w


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


def make_spin_row(label_text: str, widget) -> QWidget:
    row = QWidget(); row.setStyleSheet("background: transparent;")
    layout = QHBoxLayout(row); layout.setContentsMargins(0,2,0,2); layout.setSpacing(8)
    lbl = QLabel(label_text); lbl.setStyleSheet(STYLE_BODY); lbl.setMinimumWidth(120)
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


# ── 带开关的波纹行 ──

# ── 带开关的波纹行 ──

def ripple_row(text: str, color_hex: str) -> tuple[QWidget, _ToggleBase, QPushButton, QLabel]:
    row = QWidget(); row.setStyleSheet("background: transparent;")
    layout = QHBoxLayout(row); layout.setContentsMargins(0,2,0,2); layout.setSpacing(8)
    lbl = QLabel(text); lbl.setStyleSheet(STYLE_BODY); lbl.setMinimumWidth(120)
    layout.addWidget(lbl)
    cb = CapsuleToggle(True)
    layout.addWidget(cb)
    swatch = make_color_swatch(color_hex); label = make_color_label(color_hex)
    cpw = color_picker_widget(swatch, label); cpw.setFixedSize(140, 28)
    layout.addWidget(cpw)
    layout.addStretch()
    return row, cb, swatch, label


class TrailPanel(QWidget):
    def __init__(self, overlay):
        super().__init__()
        self.overlay = overlay
        self._controller = None
        self._app_controller = None
        self._build_ui()

    def set_controller(self, controller):
        self._controller = controller
        if not controller:
            return

        # 悬浮标实时更新
        self._cursor_combo.stateChanged.connect(
            lambda v: self._controller.set_mode(2 if v else 0)
        )
        self._cursor_outer.valueChanged.connect(
            lambda v: (self._controller.set_size(v),
                       self._cursor_inner.setMaximum(v // 2))
        )
        self._cursor_inner.valueChanged.connect(
            lambda v: self._controller.set_arc_width(v)
        )
        # 颜色选择器
        self._cursor_swatch.clicked.connect(self._pick_cursor_color)
        # 全局音量 + 波纹大小
        self._sound_volume.valueChanged.connect(
            lambda v: self._controller.set_volume(v)
        )
        self._ripple_size_spin.valueChanged.connect(
            lambda v: self._controller.set_ripple_size(v)
        )
        self._ripple_duration.valueChanged.connect(
            lambda v: self._controller.set_ripple_duration(v)
        )
        # 鼠标拖尾
        self._trail_effect_combo.currentIndexChanged.connect(
            lambda i: self._controller.set_trail_effect(None) if i == 0 else
                      self._controller.set_trail_effect(
                          list(EFFECT_REGISTRY.keys())[i - 1],
                          self._app_controller.config.data.get("trail", {}).get("trail_settings", {})
                      )
        )
        self._trail_fade_spin.valueChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_density_spin.valueChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_interval_spin.valueChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_size_spin.valueChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_opacity_spin.valueChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_color_mode.stateChanged.connect(lambda v: self._apply_trail_settings())
        self._trail_color_swatch.clicked.connect(self._pick_trail_color)
        # 声音和波纹开关即时保存 — 通过 _on_change 回调（绕过信号系统）
        for i, key in enumerate(["left","right"]):
            cb, _ = self._sound_rows[i]
            def _make_cb(k):
                def _fn(checked):
                    cfg = self._app_controller.config if self._app_controller else None
                    if cfg:
                        cfg.data.setdefault("trail", {})[f"sound_{k}_enabled"] = bool(checked)
                        cfg.save()
                return _fn
            cb._on_change = _make_cb(key)
            # 音效文件选择 → 即时保存
            combo = self._sound_combos[key]
            def _make_ss(k):
                def _fn(txt):
                    cfg = self._controller.config if self._controller else None
                    if cfg:
                        stem = self._audio_display_rev.get(txt, txt)
                        cfg.data.setdefault("trail", {})[f"sound_{k}_file"] = stem
                        cfg.save()
                return _fn
            combo.currentTextChanged.connect(_make_ss(key))
        for key in ["left","right"]:
            cb, _, _ = self._ripple_widgets[key]
            def _make_cb_ripple(k):
                def _fn(checked):
                    cfg = self._app_controller.config if self._app_controller else None
                    if cfg:
                        cfg.data.setdefault("trail", {})[f"ripple_{k}_enabled"] = bool(checked)
                        cfg.save()
                return _fn
            cb._on_change = _make_cb_ripple(key)

    def _pick_cursor_color(self):
        if not self._controller:
            return
        hex_color = self._cursor_hex if hasattr(self, '_cursor_hex') else "#ffff0000"
        current = QColor(hex_color)
        color = QColorDialog.getColor(current, self, "选择颜色",
            QColorDialog.ColorDialogOption.DontUseNativeDialog |
            QColorDialog.ColorDialogOption.ShowAlphaChannel)
        if not color.isValid():
            return
        self._cursor_swatch.setStyleSheet(
            f"QPushButton {{ background-color: {color.name(QColor.NameFormat.HexArgb)}; border:1px solid #666; border-radius:3px; }}"
        )
        self._cursor_label.setText(color.name(QColor.NameFormat.HexArgb).upper())
        self._cursor_hex = color.name(QColor.NameFormat.HexArgb)
        self._controller.set_color(color)

    def _pick_ripple_color(self, key: str, swatch: QPushButton, label: QLabel):
        """点击波纹颜色选择器"""
        if not self._app_controller or not self._app_controller.config:
            return
        default_color = "#FFFFFF00" if key == "left" else "#FF0000FF"
        old_hex = self._app_controller.config.data.setdefault("trail", {}).get(f"ripple_{key}_color", default_color)
        current = QColor(old_hex)
        color = QColorDialog.getColor(current, self, "选择颜色",
            QColorDialog.ColorDialogOption.DontUseNativeDialog |
            QColorDialog.ColorDialogOption.ShowAlphaChannel)
        if not color.isValid():
            return
        swatch.setStyleSheet(
            f"QPushButton {{ background-color: {color.name(QColor.NameFormat.HexArgb)}; border:1px solid #666; border-radius:3px; }}"
        )
        label.setText(color.name(QColor.NameFormat.HexArgb).upper())
        self._app_controller.config.data.setdefault("trail", {})[f"ripple_{key}_color"] = color.name(QColor.NameFormat.HexArgb)
        self._app_controller.config.save()

    def _pick_trail_color(self):
        """鼠标拖尾固定颜色选择器"""
        if not self._app_controller or not self._app_controller.config:
            return
        old_hex = self._app_controller.config.data.setdefault("trail", {}).get("trail_settings", {}).get("fixed_color", "#FF0000")
        current = QColor(old_hex)
        color = QColorDialog.getColor(current, self, "选择颜色")
        if not color.isValid():
            return
        self._trail_color_swatch.setStyleSheet(
            f"QPushButton {{ background-color: {color.name()}; border:1px solid #666; border-radius:3px; }}"
        )
        self._trail_color_label.setText(color.name().upper())
        # 触发一次设置应用
        self._apply_trail_settings()

    def _apply_trail_settings(self):
        """收集拖尾设置 → 控制器"""
        if not self._controller:
            return
        settings = {
            "fade_duration": self._trail_fade_spin.value() / 1000.0,
            "density": self._trail_density_spin.value(),
            "opacity": self._trail_opacity_spin.value() / 100.0,
            "spawn_interval_ms": self._trail_interval_spin.value(),
            "particle_size": self._trail_size_spin.value(),
            "max_width": self._trail_size_spin.value(),
            "color_mode": "rainbow" if self._trail_color_mode.isChecked() else "fixed",
            "fixed_color": self._trail_color_label.text() or "#FF0000",
        }
        self._controller.set_trail_settings(settings)

    def _apply_recommend(self):
        """根据当前拖尾模式设置推荐参数"""
        idx = self._trail_effect_combo.currentIndex()
        presets = {
            0: {"fade": 1000, "density": 5, "interval": 100, "size": 5, "opacity": 50, "color": "rainbow", "fixed": "#FFFF00"},
            1: {"fade": 2000, "density": 5, "interval": 20, "size": 7, "opacity": 50, "color": "rainbow", "fixed": "#FFFF00"},
            2: {"fade": 2000, "density": 5, "interval": 20, "size": 7, "opacity": 50, "color": "rainbow", "fixed": "#FFFF00"},
            3: {"fade": 10000, "density": 30, "interval": 100, "size": 10, "opacity": 100, "color": "rainbow", "fixed": "#FFFF00"},
            4: {"fade": 1000, "density": 10, "interval": 100, "size": 10, "opacity": 75, "color": "rainbow", "fixed": "#FFFF00"},
            5: {"fade": 2000, "density": 10, "interval": 100, "size": 30, "opacity": 50, "color": "rainbow", "fixed": "#FFFF00"},
        }
        p = presets.get(idx, presets[0])
        self._trail_fade_spin.setValue(p["fade"])
        self._trail_density_spin.setValue(p["density"])
        self._trail_interval_spin.setValue(p["interval"])
        self._trail_size_spin.setValue(p["size"])
        self._trail_opacity_spin.setValue(p["opacity"])
        self._trail_color_mode.setChecked(p["color"] == "rainbow")
        self._trail_color_swatch.setStyleSheet(
            f"QPushButton {{ background-color: {p['fixed']}; border:1px solid #666; border-radius:3px; }}"
        )
        self._trail_color_label.setText(p["fixed"])
        # 同步保存当前特效 ID（避免恢复默认后丢失）
        if self._app_controller and self._app_controller.config:
            eid = "pixie_dust"
            if idx > 0:
                eid = list(EFFECT_REGISTRY.keys())[idx - 1]
            self._app_controller.config.data.setdefault("trail", {})["trail_effect"] = eid
            self._app_controller.config.data["trail"]["trail_enabled"] = idx > 0
            self._app_controller.config.save()
        self._apply_trail_settings()

    def _build_ui(self):
        scroll = QScrollArea()
        scroll.setWidgetResizable(True); scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")
        scroll.verticalScrollBar().setStyleSheet(
            "QScrollBar:vertical { background:#E8E8E8; width:8px; border:none; border-radius:4px; margin:0; }"
            "QScrollBar::handle:vertical { background:#BBB; min-height:30px; border-radius:4px; }"
            "QScrollBar::handle:vertical:hover { background:#999; }"
            "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { width:0px; height:0px; border:none; background:transparent; }"
            "QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical { background:none; }"
        )
        content = QWidget()
        layout = QVBoxLayout(content)
        layout.setContentsMargins(24, 24, 24, 50); layout.setSpacing(6)

        # ═══ 1. 鼠标拖尾 ═══
        layout.addWidget(make_section(LANG.trail_effect_title))
        # 特效选择下拉（首个选项为"关闭"）
        self._trail_effect_combo = AlwaysDownCombo()
        self._trail_effect_combo.addItem("关闭")
        self._trail_effect_combo.addItems([cls.effect_name() for cls in EFFECT_REGISTRY.values()])
        self._trail_effect_combo.setItemDelegate(_ComboDel(self._trail_effect_combo))
        self._trail_effect_combo.setStyleSheet(
            "QComboBox { background:#4a9eff; color:white; border:1px solid #4a9eff; border-radius:4px; padding:4px 8px; font-size:14px; }"
            "QComboBox::drop-down { border:none; }"
        )
        layout.addWidget(make_spin_row(LANG.trail_effect_select, self._trail_effect_combo))
        # 推荐设置按钮
        self._recommend_btn = QPushButton("推荐设置")
        self._recommend_btn.setStyleSheet(
            "QPushButton { background: #4a9eff; color: white; border: 1px solid #4a9eff; "
            "border-radius: 4px; padding: 4px 12px; font-size: 14px; }"
            "QPushButton:hover { background: #3B82F6; }"
        )
        self._recommend_btn.clicked.connect(self._apply_recommend)
        rec_row = QWidget(); rec_row.setStyleSheet("background: transparent;")
        recr = QHBoxLayout(rec_row); recr.setContentsMargins(0, 2, 0, 2); recr.setSpacing(8)
        recr.addWidget(QLabel("", styleSheet=STYLE_BODY, minimumWidth=120))
        recr.addWidget(self._recommend_btn)
        recr.addStretch()
        layout.addWidget(rec_row)
        # 全局拖尾设置
        self._trail_fade_spin = QSpinBox(); self._trail_fade_spin.setRange(100, 10000); self._trail_fade_spin.setValue(1000); self._trail_fade_spin.setSuffix(" ms"); self._trail_fade_spin.setSingleStep(100)
        layout.addWidget(make_spin_row("淡出时长", self._trail_fade_spin))
        self._trail_density_spin = QSpinBox(); self._trail_density_spin.setRange(1, 50); self._trail_density_spin.setValue(6)
        layout.addWidget(make_spin_row("粒子密度", self._trail_density_spin))
        self._trail_interval_spin = QSpinBox(); self._trail_interval_spin.setRange(10, 1000); self._trail_interval_spin.setValue(20); self._trail_interval_spin.setSuffix(" ms"); self._trail_interval_spin.setSingleStep(10)
        layout.addWidget(make_spin_row("生成间隔", self._trail_interval_spin))
        self._trail_size_spin = QSpinBox(); self._trail_size_spin.setRange(1, 50); self._trail_size_spin.setValue(8); self._trail_size_spin.setSuffix(" px")
        layout.addWidget(make_spin_row("粒子大小", self._trail_size_spin))
        self._trail_opacity_spin = QSpinBox(); self._trail_opacity_spin.setRange(10, 100); self._trail_opacity_spin.setValue(80); self._trail_opacity_spin.setSuffix(" %")
        layout.addWidget(make_spin_row("不透明度", self._trail_opacity_spin))
        # 颜色模式 — 双按钮切换
        self._trail_color_mode = ToggleSwitch()
        self._trail_color_mode.setChecked(True)
        self._trail_color_mode._on_btn.setText("彩虹")
        self._trail_color_mode._off_btn.setText("单色")
        self._trail_color_mode.setFixedSize(160, 28)
        color_mode_row = QWidget(); color_mode_row.setStyleSheet("background: transparent;")
        cmr = QHBoxLayout(color_mode_row); cmr.setContentsMargins(0,2,0,2); cmr.setSpacing(8)
        cm_label = QLabel("颜色模式"); cm_label.setStyleSheet(STYLE_BODY); cm_label.setMinimumWidth(120)
        cmr.addWidget(cm_label)
        cmr.addWidget(self._trail_color_mode)
        cmr.addStretch()
        layout.addWidget(color_mode_row)
        # 固定颜色
        self._trail_color_swatch = make_color_swatch("#FF0000"); self._trail_color_label = make_color_label("#FF0000")
        cpw_trail = color_picker_widget(self._trail_color_swatch, self._trail_color_label); cpw_trail.setFixedSize(160, 28)
        layout.addWidget(make_spin_row("单色颜色", cpw_trail))

        # ═══ 2. 悬浮标 ═══
        layout.addWidget(make_section(LANG.trail_cursor))
        self._cursor_combo = ToggleSwitch()
        self._cursor_combo.setChecked(False)
        self._cursor_combo.setFixedSize(160, 28)
        cursor_status_row = QWidget(); cursor_status_row.setStyleSheet("background: transparent;")
        csr = QHBoxLayout(cursor_status_row); csr.setContentsMargins(0,2,0,2); csr.setSpacing(8)
        cs_label = QLabel(LANG.trail_cursor_status); cs_label.setStyleSheet(STYLE_BODY); cs_label.setMinimumWidth(120)
        csr.addWidget(cs_label)
        csr.addWidget(self._cursor_combo)
        csr.addStretch()
        layout.addWidget(cursor_status_row)

        self._cursor_swatch = make_color_swatch("#80ff0000"); self._cursor_label = make_color_label("#80ff0000")
        self._cursor_hex = "#ff0000ff"
        cpw = color_picker_widget(self._cursor_swatch, self._cursor_label); cpw.setFixedSize(160, 28)
        layout.addWidget(make_spin_row(LANG.trail_cursor_color, cpw))

        self._cursor_outer = QSpinBox(); self._cursor_outer.setRange(10,400); self._cursor_outer.setValue(60); self._cursor_outer.setSuffix(" px")
        layout.addWidget(make_spin_row(LANG.trail_cursor_outer, self._cursor_outer))
        self._cursor_inner = QSpinBox(); self._cursor_inner.setRange(1, 199); self._cursor_inner.setValue(5); self._cursor_inner.setSuffix(" px")
        layout.addWidget(make_spin_row(LANG.trail_cursor_inner, self._cursor_inner))

        # ═══ 3. 点击声音 ═══
        layout.addWidget(make_section(LANG.trail_sound))
        # 全局音量
        self._sound_volume = QSpinBox(); self._sound_volume.setRange(0, 100); self._sound_volume.setValue(50); self._sound_volume.setSuffix(" %")
        layout.addWidget(make_spin_row(LANG.trail_sound_volume, self._sound_volume))
        import os as _os
        _audio_dir = _os.path.join(_os.path.dirname(_os.path.dirname(_os.path.dirname(_os.path.abspath(__file__)))), "resources", "audio")
        _audio_files = sorted(
            [_f for _f in _os.listdir(_audio_dir) if _f.endswith(".wav")]
        )
        _stems = [f.replace(".wav","") for f in _audio_files]
        # 从 mapping.txt 读取显示名，不存在则使用 stem
        _audio_display = {}
        _audio_display_rev = {}
        _map_path = _os.path.join(_audio_dir, "mapping.txt")
        if _os.path.exists(_map_path):
            with open(_map_path, "r", encoding="utf-8") as _mf:
                for _line in _mf:
                    _line = _line.strip()
                    if not _line or _line.startswith("#"):
                        continue
                    if "=" in _line:
                        _stem, _disp = _line.split("=", 1)
                        _audio_display[_stem.strip()] = _disp.strip()
                        _audio_display_rev[_disp.strip()] = _stem.strip()
        # 未映射的 stem 直接用作显示
        for s in _stems:
            if s not in _audio_display:
                _audio_display[s] = s
                _audio_display_rev[s] = s
        self._sound_rows = []
        self._sound_combos = {}
        for key in ["left","right"]:
            lt = getattr(LANG, f"trail_sound_{key}")
            row = QWidget(); row.setStyleSheet("background:transparent;")
            rl = QHBoxLayout(row); rl.setContentsMargins(0,2,0,2); rl.setSpacing(8)
            lbl = QLabel(lt); lbl.setStyleSheet(STYLE_BODY); lbl.setMinimumWidth(120)
            rl.addWidget(lbl)
            cb = CapsuleToggle(True)
            rl.addWidget(cb)
            combo = AlwaysDownCombo()
            # 显示名列表，内部用 display → stem 映射
            _stems = [f.replace(".wav","") for f in _audio_files]
            _display_list = [_audio_display[s] for s in _stems]
            combo.addItems(_display_list)
            combo.blockSignals(True)
            combo.setCurrentText(_audio_display.get("mouseclick1", "mouseclick1"))
            combo.blockSignals(False)
            combo.setItemDelegate(_ComboDel(combo))
            combo.setStyleSheet(
                "QComboBox { background:#4a9eff; color:white; border:1px solid #4a9eff; border-radius:4px; padding:4px 6px; font-size:14px; }"
                "QComboBox::drop-down { border:none; }"
            )
            combo.setFixedSize(110, 28)
            rl.addWidget(combo); rl.addStretch()
            layout.addWidget(row)
            self._sound_rows.append((cb, combo))
            self._sound_combos[key] = combo
        self._audio_display = _audio_display
        self._audio_display_rev = _audio_display_rev

        # ═══ 4. 点击波纹 ═══
        layout.addWidget(make_section("点击波纹"))
        # 全局波纹大小
        self._ripple_size_spin = QSpinBox(); self._ripple_size_spin.setRange(10, 500); self._ripple_size_spin.setValue(100); self._ripple_size_spin.setSuffix(" px")
        layout.addWidget(make_spin_row(LANG.trail_ripple_size, self._ripple_size_spin))
        self._ripple_duration = QSpinBox(); self._ripple_duration.setRange(100, 3000); self._ripple_duration.setValue(300); self._ripple_duration.setSuffix(" ms"); self._ripple_duration.setSingleStep(100)
        layout.addWidget(make_spin_row(LANG.trail_ripple_duration, self._ripple_duration))
        self._ripple_widgets = {}
        colors = {"left":"#FFFFFF00","right":"#FF0000FF"}
        for i, key in enumerate(["left","right"]):
            lt = getattr(LANG, f"trail_ripple_{key}")
            row, cb, swatch, label = ripple_row(lt, colors[key])
            swatch.clicked.connect(lambda checked=None, k=key, s=swatch, l=label:
                self._pick_ripple_color(k, s, l))
            layout.addWidget(row)
            self._ripple_widgets[key] = (cb, swatch, label)


        layout.addStretch()
        scroll.setWidget(content)
        outer = QVBoxLayout(self); outer.setContentsMargins(0,0,0,0); outer.addWidget(scroll)

    def load_settings(self, settings: dict):
        """从配置数据同步全部 UI 控件"""
        data = settings.get("trail", {})
        for child in self.findChildren(QWidget):
            child.blockSignals(True)
        self._cursor_combo.setChecked(data.get("cursor_mode", 0) != 0)
        self._cursor_outer.setValue(data["cursor_outer"])
        self._cursor_inner.setValue(data["cursor_arc"])
        # 颜色
        cc = data["cursor_color"]
        qc = QColor(cc)
        if qc.isValid():
            self._cursor_label.setText(qc.name(QColor.NameFormat.HexArgb).upper())
            self._cursor_hex = qc.name(QColor.NameFormat.HexArgb)
        else:
            self._cursor_label.setText(cc)
        self._cursor_swatch.setStyleSheet(
            f"QPushButton {{ background-color: {cc}; border:1px solid #666; border-radius:3px; }}"
        )
        # 全局音量 + 波纹大小
        self._sound_volume.setValue(data.get("sound_volume", 50))
        self._ripple_size_spin.setValue(data.get("ripple_size", 100))
        self._ripple_duration.setValue(data.get("ripple_duration", 1000))
        # 鼠标拖尾
        effect_id = data.get("trail_effect", "pixie_dust")
        enabled = data.get("trail_enabled", False)
        if enabled and effect_id in EFFECT_REGISTRY:
            idx = list(EFFECT_REGISTRY.keys()).index(effect_id) + 1
        else:
            idx = 0
        self._trail_effect_combo.setCurrentIndex(idx)
        # 拖尾全局设置
        ts = data.get("trail_settings", {})
        self._trail_fade_spin.setValue(int(ts.get("fade_duration", 1.0) * 1000))
        self._trail_density_spin.setValue(ts.get("density", 5))
        self._trail_interval_spin.setValue(ts.get("spawn_interval_ms", 100))
        self._trail_size_spin.setValue(ts.get("particle_size", 5))
        self._trail_opacity_spin.setValue(int(ts.get("opacity", 0.5) * 100))
        self._trail_color_mode.setChecked(ts.get("color_mode", "rainbow") == "rainbow")
        fc = ts.get("fixed_color", "#FFFF00")
        self._trail_color_swatch.setStyleSheet(
            f"QPushButton {{ background-color: {fc}; border:1px solid #666; border-radius:3px; }}"
        )
        self._trail_color_label.setText(fc.upper())
        # 声音开关 + 音效文件
        if hasattr(self, '_sound_rows'):
            for i, key in enumerate(["left","right"]):
                self._sound_rows[i][0].setChecked(data[f"sound_{key}_enabled"])
                disp = data.get(f"sound_{key}_file", "mouseclick1")
                if hasattr(self, '_audio_display'):
                    disp = self._audio_display.get(disp, disp)
                self._sound_rows[i][1].setCurrentText(disp)
        # 波纹开关 + 颜色
        if hasattr(self, '_ripple_widgets'):
            for key in ["left","right"]:
                self._ripple_widgets[key][0].setChecked(data[f"ripple_{key}_enabled"])
                default_color = "#FFFFFF00" if key == "left" else "#FF0000FF"
                hex_color = data.get(f"ripple_{key}_color", default_color)
                swatch, label = self._ripple_widgets[key][1], self._ripple_widgets[key][2]
                swatch.setStyleSheet(
                    f"QPushButton {{ background-color: {hex_color}; border:1px solid #666; border-radius:3px; }}"
                )
                label.setText(hex_color.upper())
        for child in self.findChildren(QWidget):
            child.blockSignals(False)
        # 若已选择特效（非关闭），手动启动
        if self._controller and self._trail_effect_combo.currentIndex() > 0:
            eid = list(EFFECT_REGISTRY.keys())[self._trail_effect_combo.currentIndex() - 1]
            ts = data.get("trail_settings", {})
            self._controller.set_trail_effect(eid, ts)

    def to_settings(self) -> dict:
        result = {"cursor_mode": 2 if self._cursor_combo.isChecked() else 0, "cursor_outer": self._cursor_outer.value(),
                  "cursor_inner": self._cursor_inner.value(), "cursor_color": self._cursor_label.text()}
        for key, (cb, swatch, label) in self._ripple_widgets.items():
            result[f"ripple_{key}_enabled"] = cb.isChecked()
            result[f"ripple_{key}_color"] = label.text()
        for i, key in enumerate(["left","right","middle","back","forward"]):
            cb, _ = self._sound_rows[i]
            result[f"sound_{key}_enabled"] = cb.isChecked()
        return result
