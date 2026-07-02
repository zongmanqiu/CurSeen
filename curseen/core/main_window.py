"""统一主窗口：左侧功能切换 + 右侧控制面板"""

from PySide6.QtWidgets import (
    QMainWindow, QWidget, QHBoxLayout, QVBoxLayout,
    QListWidget, QStackedWidget, QLabel,
)
from PySide6.QtCore import Qt
from PySide6.QtGui import QIcon, QPixmap
from PySide6.QtCore import QByteArray

from curseen.core.lang import LANG
from curseen.core.style import PANEL_BG, NAV_BG_STYLE, NAV_LIST_STYLE
from curseen.trail.panel import TrailPanel
from curseen.key_display.panel import KeyDisplayPanel
from curseen.drawing.panel import DrawingPanel
from curseen.core.global_panel import GlobalPanel
from curseen.about.panel import AboutPanel
from resources.icons import LOGO_SVG


class MainWindow(QMainWindow):
    """统一主窗口，侧边栏切换功能面板"""

    NAV_ITEMS = [
        LANG.nav_trail,
        LANG.nav_key,
        LANG.nav_draw,
        LANG.nav_global,
        LANG.nav_about,
    ]

    def __init__(self, controller, overlay):
        super().__init__()
        self.controller = controller
        self.overlay = overlay
        self._setup_ui()

    def _setup_ui(self):
        self.setWindowTitle(LANG.window_title)
        from curseen.core.tray import _make_logo_icon
        self.setWindowIcon(_make_logo_icon())

        self.resize(680, 520)
        self.setMinimumSize(480, 400)

        central = QWidget()
        self.setCentralWidget(central)
        layout = QHBoxLayout(central)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        # 左侧导航栏
        nav_widget = QWidget()
        nav_widget.setStyleSheet(NAV_BG_STYLE)
        nav_layout = QVBoxLayout(nav_widget)
        nav_layout.setContentsMargins(0, 0, 0, 0)
        nav_layout.setSpacing(0)

        self.nav = QListWidget()
        self.nav.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self.nav.setStyleSheet(NAV_LIST_STYLE)
        for item in self.NAV_ITEMS:
            self.nav.addItem(item)
        # 导航宽度自适应最长条目
        from curseen.core.style import get_font_size
        fs = get_font_size()
        max_chars = max(len(t) for t in self.NAV_ITEMS)
        nav_widget.setFixedWidth(int(max_chars * fs * 0.65 + 55))
        nav_layout.addWidget(self.nav, stretch=1)

        # 右侧面板容器
        self.panels = QStackedWidget()
        self.panels.setStyleSheet(f"QWidget {{ {PANEL_BG} }}")

        self._trail_panel = TrailPanel(self.overlay)
        if self.controller and hasattr(self.controller, 'trail_controller'):
            self._trail_panel.set_controller(self.controller.trail_controller)
            self._trail_panel._app_controller = self.controller
            if hasattr(self.controller, 'config'):
                self._trail_panel.load_settings(self.controller.config.data)
        self._key_panel = KeyDisplayPanel(self.overlay)
        self._draw_panel = DrawingPanel(self.overlay, getattr(self.controller, 'drawing_controller', None))
        self._global_panel = GlobalPanel()
        if self.controller and hasattr(self.controller, 'config'):
            gs = self.controller.config.data.get("global", {})
            self._global_panel.load_settings(gs)
            self._global_panel.set_controller(self.controller)
        self._about_panel = AboutPanel()

        # 传递按键控制器
        if self.controller and hasattr(self.controller, 'key_controller'):
            self._key_panel.set_controller(self.controller.key_controller)
            self._key_panel._app_controller = self.controller
            self._key_panel._sync_from_controller()

        self.panels.addWidget(self._trail_panel)
        self.panels.addWidget(self._key_panel)
        self.panels.addWidget(self._draw_panel)
        self.panels.addWidget(self._global_panel)
        self.panels.addWidget(self._about_panel)

        layout.addWidget(nav_widget)
        layout.addWidget(self.panels, stretch=1)

        self.nav.currentRowChanged.connect(self.panels.setCurrentIndex)
        self.nav.setCurrentRow(0)

        # ── 启动最终统一同步：配置 → 全部面板 UI ──
        if self.controller and hasattr(self.controller, 'config'):
            cfg = self.controller.config.data
            self._global_panel.load_settings({"global": cfg.get("global", {})})
            if hasattr(self._key_panel, '_sync_from_controller'):
                self._key_panel._sync_from_controller()
            if hasattr(self._trail_panel, 'load_settings'):
                self._trail_panel.load_settings({"trail": cfg.get("trail", {})})
