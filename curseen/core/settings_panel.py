"""全局设置面板（占位）"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QLabel, QCheckBox, QLineEdit, QGroupBox,
)

from curseen.core.lang import LANG
from curseen.core.style import CHECKBOX_STYLE, GROUPBOX_STYLE, INPUT_STYLE, BODY_STYLE


class SettingsPanel(QWidget):
    """全局设置面板（主窗口右侧）"""

    def __init__(self, controller):
        super().__init__()
        self.controller = controller
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.setSpacing(12)

        general = QGroupBox(LANG.settings_general)
        general.setStyleSheet(GROUPBOX_STYLE)
        gl = QVBoxLayout(general)
        self.auto_start_cb = QCheckBox(LANG.settings_auto_start)
        self.auto_start_cb.setStyleSheet(CHECKBOX_STYLE)
        gl.addWidget(self.auto_start_cb)
        layout.addWidget(general)

        hotkey = QGroupBox(LANG.settings_hotkey)
        hotkey.setStyleSheet(GROUPBOX_STYLE)
        hl = QVBoxLayout(hotkey)
        hl.addWidget(QLabel(LANG.settings_toggle_draw, styleSheet=BODY_STYLE))
        self.hotkey_edit = QLineEdit(LANG.settings_default_hotkey)
        self.hotkey_edit.setStyleSheet(INPUT_STYLE)
        hl.addWidget(self.hotkey_edit)
        layout.addWidget(hotkey)

        layout.addStretch()
