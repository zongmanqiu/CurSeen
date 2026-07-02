"""系统托盘图标"""

from PySide6.QtWidgets import QSystemTrayIcon, QMenu
from PySide6.QtGui import QIcon, QPixmap, QPainter
from PySide6.QtCore import Qt, QByteArray

from curseen.core.lang import LANG
from resources.icons import LOGO_SVG


def _make_logo_icon() -> QIcon:
    """从内嵌 SVG base64 生成图标（使用 QSvgRenderer，不依赖 SVG 图像插件）"""
    from PySide6.QtSvg import QSvgRenderer
    from PySide6.QtCore import QByteArray
    data = QByteArray.fromBase64(LOGO_SVG.encode())
    renderer = QSvgRenderer(data)
    if not renderer.isValid():
        pixmap = QPixmap(16, 16)
        pixmap.fill(Qt.GlobalColor.transparent)
    else:
        default_size = renderer.defaultSize()
        sz = max(default_size.width(), default_size.height(), 64)
        pixmap = QPixmap(sz, sz)
        pixmap.fill(Qt.GlobalColor.transparent)
        painter = QPainter(pixmap)
        renderer.render(painter)
        painter.end()
    return QIcon(pixmap)


class TrayManager:
    """负责系统托盘的显示/隐藏/退出"""

    def __init__(self, controller):
        self.controller = controller
        self.tray = QSystemTrayIcon()
        self.tray.setIcon(_make_logo_icon())
        self._setup_menu()

    def _setup_menu(self):
        self.tray.setToolTip(LANG.tray_tooltip)
        menu = QMenu()
        menu.addAction(LANG.tray_quit, lambda: self.controller.quit())
        self.tray.setContextMenu(menu)
        self.tray.activated.connect(self._on_activated)

    def _on_activated(self, reason):
        if reason == QSystemTrayIcon.ActivationReason.DoubleClick:
            self.controller.show_window()

    def show(self):
        self.tray.show()

    def hide(self):
        self.tray.hide()
