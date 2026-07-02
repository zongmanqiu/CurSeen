"""全局设置面板 —— 快捷键捕获 + 功能开关"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QLabel, QCheckBox, QHBoxLayout,
    QPushButton, QButtonGroup, QLineEdit, QApplication, QSizePolicy,
)
from PySide6.QtCore import Qt, Signal, QEvent

from curseen.core.lang import LANG
from curseen.core.style import STYLE_BODY, STYLE_SECTION, STYLE_CHECKBOX


_STYLE_IDLE = (
    "QLineEdit { background: #4a9eff; color: white; border: 1px solid #4a9eff; "
    "border-radius: 4px; padding: 4px 8px; font-size: 14px; }"
)
_STYLE_CAPTURE = (
    "QLineEdit { background: #F59E0B; color: white; border: 1px solid #D97706; "
    "border-radius: 4px; padding: 4px 8px; font-size: 14px; }"
)


class KeyCaptureLineEdit(QLineEdit):
    """快捷键捕获输入框"""

    hotkey_changed = Signal(str)

    def __init__(self, parent=None):
        super().__init__(parent)
        self._hotkey: str = ""
        self._capturing = False
        self._mods: set[str] = set()
        self._key: str | None = None
        self._require_modifier = False  # True=必须组合键/功能键, False=允许单键

        self.setReadOnly(True)
        self.setText("单击以捕获快捷键")
        self.setStyleSheet(_STYLE_IDLE)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
        QApplication.instance().installEventFilter(self)

    # ── 事件 ──

    def eventFilter(self, obj, event):
        if event.type() == QEvent.Type.MouseButtonPress and obj is not self:
            self._exit_capture()
        return super().eventFilter(obj, event)

    def mousePressEvent(self, event):
        """点击 → 清空快捷键 → 进入捕获（橙黄色）"""
        self._capturing = True
        self._hotkey = ""
        self._mods.clear()
        self._key = None
        self.setText("单击以捕获快捷键")
        self.setStyleSheet(_STYLE_CAPTURE)
        self.setFocus()
        self.hotkey_changed.emit("")
        super().mousePressEvent(event)

    def focusOutEvent(self, event):
        self._exit_capture()
        super().focusOutEvent(event)

    # ── 桥接 ──

    def handle_pynput_key(self, key_str: str):
        """从 pynput 接收按键"""
        if not self._capturing:
            return
        from curseen.key_display.keymap import lookup
        r = lookup(key_str)
        if r is None:
            return
        display, ktype, _ = r
        if ktype == "mod":
            self._mods.add(display)
            return
        if display:
            # 单字符统一大写（避免 Alt+B 和 Ctrl+B 大小写不一致）
            if len(display) == 1:
                display = display.upper()
            self._key = display
            self._finish_capture()

    # ── 内部 ──

    def _finish_capture(self):
        if not self._key:
            return
        if self._require_modifier and not self._mods and len(self._key) == 1 and self._key.isalnum():
            self.clear()
            return
        parts = []
        for mod in ("Ctrl", "Alt", "Shift", "Win"):
            if mod in self._mods:
                parts.append(mod)
        parts.append(self._key)
        self._hotkey = " + ".join(parts)
        self.setText(self._hotkey)
        self._capturing = False
        self.setStyleSheet(_STYLE_IDLE)
        self.hotkey_changed.emit(self._hotkey)
        self.clearFocus()

    def _exit_capture(self):
        if not self._capturing:
            return
        self._capturing = False
        self._mods.clear()
        self._key = None
        self.setStyleSheet(_STYLE_IDLE)
        if not self._hotkey:
            self.setText("单击以捕获快捷键")

    # ── 外部接口 ──

    def set_hotkey(self, hotkey: str):
        self._hotkey = hotkey
        self.setText(hotkey if hotkey else "单击以捕获快捷键")

    def hotkey(self) -> str:
        return self._hotkey


class ToggleSwitch(QWidget):
    """双按钮开关：开启 / 关闭"""

    toggled = Signal(bool)
    stateChanged = Signal(int)

    def __init__(self, parent=None):
        super().__init__(parent)
        self._checked = True
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        self._on_btn = QPushButton("开启")
        self._off_btn = QPushButton("关闭")
        for btn in (self._on_btn, self._off_btn):
            btn.setCheckable(True)
            btn.setFixedHeight(28)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)

        self._group = QButtonGroup(self)
        self._group.addButton(self._on_btn, 1)
        self._group.addButton(self._off_btn, 0)
        self._group.setExclusive(True)
        self._group.idToggled.connect(self._on_toggled)

        layout.addWidget(self._on_btn)
        layout.addWidget(self._off_btn)
        self._on_btn.setChecked(True)
        self._update_style()

    def _update_style(self):
        for btn, active in [(self._on_btn, self._checked), (self._off_btn, not self._checked)]:
            if active:
                btn.setStyleSheet(
                    f"QPushButton {{ background: #4a9eff; color: white; border: 1px solid #4a9eff; "
                    f"padding: 4px 16px; font-size: 13px; border-radius: 0; }}"
                )
            else:
                btn.setStyleSheet(
                    f"QPushButton {{ background: white; color: #000; border: 1px solid #4a9eff; "
                    f"padding: 4px 16px; font-size: 13px; border-radius: 0; }}"
                )
        self._on_btn.setStyleSheet(
            self._on_btn.styleSheet() +
            "QPushButton { border-top-left-radius: 4px; border-bottom-left-radius: 4px; "
            "border-right: none; }"
        )
        self._off_btn.setStyleSheet(
            self._off_btn.styleSheet() +
            "QPushButton { border-top-right-radius: 4px; border-bottom-right-radius: 4px; "
            "border-left: none; }"
        )

    def _on_toggled(self, btn_id: int, checked: bool):
        if checked:
            self._checked = (btn_id == 1)
            self._update_style()
            self.toggled.emit(self._checked)
            self.stateChanged.emit(2 if self._checked else 0)

    def setChecked(self, checked: bool):
        self._checked = checked
        if checked:
            self._on_btn.setChecked(True)
        else:
            self._off_btn.setChecked(True)
        self._update_style()
        self.toggled.emit(self._checked)
        self.stateChanged.emit(2 if self._checked else 0)

    def isChecked(self) -> bool:
        return self._checked


class GlobalPanel(QWidget):
    """全局设置面板"""

    settings_changed = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self._build_ui()

    def _build_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 50)
        layout.setSpacing(6)

        # ── 快捷键 ──
        shortcut_label = QLabel(LANG.global_panel_hotkey)
        shortcut_label.setStyleSheet(STYLE_SECTION)
        shortcut_row = QWidget()
        shortcut_row.setStyleSheet("background: transparent;")
        shortcut_row.setFixedHeight(32)
        sr_layout = QHBoxLayout(shortcut_row)
        sr_layout.setContentsMargins(0, 2, 0, 2)
        sr_layout.addWidget(shortcut_label)
        sr_layout.addStretch()
        layout.addWidget(shortcut_row)

        hk_row = QWidget()
        hk_row.setStyleSheet("background: transparent;")
        hk_row.setFixedHeight(32)
        hk_row_layout = QHBoxLayout(hk_row)
        hk_row_layout.setContentsMargins(0, 2, 0, 2)
        hk_row_layout.setSpacing(8)
        panel_hk_label = QLabel("软件面板")
        panel_hk_label.setStyleSheet(STYLE_BODY)
        panel_hk_label.setMinimumWidth(120)
        hk_row_layout.addWidget(panel_hk_label)
        self._key_capture = KeyCaptureLineEdit()
        self._key_capture.setFixedSize(160, 28)
        hk_row_layout.addWidget(self._key_capture)
        hk_row_layout.addStretch()
        layout.addWidget(hk_row)

        # ── 功能开关 ──
        toggle_label = QLabel(LANG.global_feature_toggle)
        toggle_label.setStyleSheet(STYLE_SECTION)
        toggle_row = QWidget()
        toggle_row.setStyleSheet("background: transparent;")
        toggle_row.setFixedHeight(32)
        tr_layout = QHBoxLayout(toggle_row)
        tr_layout.setContentsMargins(0, 2, 0, 2)
        tr_layout.addWidget(toggle_label)
        tr_layout.addStretch()
        layout.addWidget(toggle_row)

        self._trail_cb = self._make_toggle()
        self._key_cb = self._make_toggle()
        self._draw_cb = self._make_toggle()

        for text, cb in [
            (LANG.global_toggle_trail, self._trail_cb),
            (LANG.global_toggle_key, self._key_cb),
            (LANG.global_toggle_draw, self._draw_cb),
        ]:
            row = QWidget()
            row.setStyleSheet("background: transparent;")
            row.setFixedHeight(32)
            row_layout = QHBoxLayout(row)
            row_layout.setContentsMargins(0, 2, 0, 2)
            row_layout.setSpacing(8)
            label = QLabel(text)
            label.setStyleSheet(STYLE_BODY)
            label.setMinimumWidth(120)
            row_layout.addWidget(label)
            cb.setFixedSize(160, 28)
            row_layout.addWidget(cb)
            row_layout.addStretch()
            layout.addWidget(row)

        # ── 其它 ──
        other_section = QWidget()
        other_section.setStyleSheet("background: transparent;")
        other_section.setFixedHeight(32)
        os_layout = QHBoxLayout(other_section)
        os_layout.setContentsMargins(0, 2, 0, 2)
        os_layout.addWidget(QLabel("其它", styleSheet=STYLE_SECTION))
        os_layout.addStretch()
        layout.addWidget(other_section)

        # 恢复默认设置
        reset_btn = QPushButton("恢复默认设置")
        reset_btn.setStyleSheet(
            "QPushButton { background: #E74C3C; color: white; border: 1px solid #C0392B; "
            "border-radius: 4px; padding: 4px 16px; font-size: 14px; }"
            "QPushButton:hover { background: #C0392B; }"
        )
        reset_btn.setFixedSize(160, 28)
        reset_btn.clicked.connect(self._reset_defaults)
        reset_row = QWidget()
        reset_row.setStyleSheet("background: transparent;")
        reset_row.setFixedHeight(32)
        reset_row_layout = QHBoxLayout(reset_row)
        reset_row_layout.setContentsMargins(0, 2, 0, 2)
        reset_row_layout.setSpacing(8)
        reset_row_layout.addWidget(QLabel("默认设置", styleSheet=STYLE_BODY, minimumWidth=120))
        reset_row_layout.addWidget(reset_btn)
        reset_row_layout.addStretch()
        layout.addWidget(reset_row)

        layout.addStretch()

    def _reset_defaults(self):
        """恢复所有设置为默认值"""
        from PySide6.QtWidgets import QMessageBox
        reply = QMessageBox.question(
            self, "确认重置",
            "确定要恢复所有设置为默认值吗？\n此操作不可撤销。",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
            QMessageBox.StandardButton.No,
        )
        if reply != QMessageBox.StandardButton.Yes:
            return

        if not self._controller or not self._controller.config:
            return
        cfg = self._controller.config
        ctrl = self._controller

        # 1. 删除 config.json + 内存重置为 DEFAULT_CONFIG
        cfg.reset_to_defaults()

        # 2. 全局设置面板 UI — 直接操纵底层按钮，绕过 ToggleSwitch 信号
        self._key_capture.set_hotkey("")
        for cb in (self._trail_cb, self._key_cb, self._draw_cb):
            cb._checked = False
            cb._on_btn.setChecked(False)
            cb._off_btn.setChecked(True)
            cb._update_style()
        cfg.data["global"]["panel_hotkey"] = ""
        cfg.data["global"]["enabled_trail"] = False
        cfg.data["global"]["enabled_key"] = False
        cfg.data["global"]["enabled_draw"] = False

        mw = ctrl.main_window

        # 3. 按键显示
        if ctrl.key_controller:
            ctrl.key_controller.load_settings(cfg.data.get("key_display", {}))
            if mw and hasattr(mw, '_key_panel'):
                mw._key_panel._sync_from_controller()

        # 4. 鼠标特效（load_settings 内有 blockSignals，不会写盘）
        if ctrl.trail_controller:
            ctrl.trail_controller.load_settings({"trail": cfg.data.get("trail", {})})
            ctrl.trail_controller.set_trail_enabled(False)  # 强制关闭特效
            if mw and hasattr(mw, '_trail_panel'):
                mw._trail_panel.load_settings({"trail": cfg.data.get("trail", {})})

        # 4b. 屏幕书写
        if mw and hasattr(mw, '_draw_panel'):
            mw._draw_panel.load_settings(cfg.data)

        # 5. 写盘保存当前内存配置
        cfg.save()

        # 6. 覆盖层
        if ctrl.overlay:
            ctrl.overlay.update()

    def set_controller(self, controller):
        self._controller = controller
        if not controller:
            return

        def _save_toggle(key):
            def handler(checked):
                if self._controller and self._controller.config:
                    self._controller.config.data.setdefault("global", {})[key] = bool(checked)
                    self._controller.config.save()
            return handler

        self._trail_cb.stateChanged.connect(lambda checked: (
            controller.trail_controller.set_trail_enabled(bool(checked)) if controller.trail_controller else None,
            _save_toggle("enabled_trail")(checked),
        ))
        self._key_cb.stateChanged.connect(lambda checked: (
            setattr(controller.key_controller, 'enabled', bool(checked)),
            _save_toggle("enabled_key")(checked)
        ))
        self._draw_cb.stateChanged.connect(lambda checked: (
            setattr(controller.drawing_controller, '_enabled', bool(checked)),
            _save_toggle("enabled_draw")(checked),
        ))
        self._key_capture.hotkey_changed.connect(lambda hk: (
            controller._parse_hotkey(hk),
            controller.config.set_and_save(["global", "panel_hotkey"], hk)
        ))

    @staticmethod
    def _make_toggle() -> QWidget:
        return ToggleSwitch()

    def load_settings(self, settings: dict):
        gs = settings.get("global", {})
        self._key_capture.set_hotkey(gs.get("panel_hotkey", ""))
        self._trail_cb.setChecked(gs.get("enabled_trail", False))
        self._key_cb.setChecked(gs.get("enabled_key", False))
        self._draw_cb.setChecked(gs.get("enabled_draw", False))

    def to_settings(self) -> dict:
        return {
            "panel_hotkey": self._key_capture.hotkey(),
            "enabled_trail": self._trail_cb.isChecked(),
            "enabled_key": self._key_cb.isChecked(),
            "enabled_draw": self._draw_cb.isChecked(),
        }
