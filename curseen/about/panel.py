"""关于页面 — 显示项目信息"""

from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QFrame, QPushButton,
    QScrollArea,
)
from PySide6.QtCore import Qt, QByteArray
from PySide6.QtGui import QPixmap

from curseen import (
    __version__, __author__, __author_url__,
    __email__, __url__, __tools__,
)
from curseen.core.lang import LANG
from curseen.core.style import (
    STYLE_BODY, STYLE_H1, STYLE_LINK,
    C_WHITE, C_LINK, C_BG,
)
from resources.icons import LOGO_SVG
from resources.icons.qr_codes import WECHAT_QR, ALIPAY_QR


class AboutPanel(QWidget):
    """关于 CurSeen / 可寻"""

    LABEL_WIDTH = 80
    LABEL_ALIGN = Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter

    def __init__(self):
        super().__init__()
        self._setup_ui()

    @staticmethod
    def _load_logo() -> QPixmap:
        data = QByteArray.fromBase64(LOGO_SVG.encode())
        pixmap = QPixmap()
        pixmap.loadFromData(data, "SVG")
        return pixmap

    def _make_label(self, text: str) -> QLabel:
        lbl = QLabel(text + "：")
        lbl.setFixedWidth(self.LABEL_WIDTH)
        lbl.setStyleSheet(STYLE_BODY)
        lbl.setAlignment(self.LABEL_ALIGN)
        return lbl

    def _make_link_row(self, label: str, link_text: str, url: str) -> QWidget:
        row = QWidget()
        row.setStyleSheet("background: transparent;")
        row.setFixedHeight(32)
        layout = QHBoxLayout(row)
        layout.setContentsMargins(0, 2, 0, 2)
        layout.setSpacing(8)
        layout.addWidget(self._make_label(label))
        link = QLabel(
            f'<a href="{url}" style="color:{C_LINK};text-decoration:none;">{link_text}</a>'
        )
        link.setTextFormat(Qt.TextFormat.RichText)
        link.setOpenExternalLinks(True)
        link.setStyleSheet(STYLE_BODY)
        layout.addWidget(link, stretch=1)
        return row

    def _make_text_row(self, label: str, text: str) -> QWidget:
        row = QWidget()
        row.setStyleSheet("background: transparent;")
        row.setFixedHeight(32)
        layout = QHBoxLayout(row)
        layout.setContentsMargins(0, 2, 0, 2)
        layout.setSpacing(8)
        layout.addWidget(self._make_label(label))
        val = QLabel(text)
        val.setStyleSheet(STYLE_BODY)
        layout.addWidget(val, stretch=1)
        return row

    def _on_check_update(self):
        """检查更新流程"""
        from curseen.update.checker import VersionChecker
        from curseen.update.downloader import ZipDownloader
        from curseen.update.installer import UpdateInstaller
        from PySide6.QtWidgets import QMessageBox, QProgressDialog
        import os, tempfile

        self._update_btn.setEnabled(False)
        self._update_btn.setText(LANG.about_update_checking)

        self._checker = VersionChecker(__version__, self)

        def on_check(result):
            if result["error"]:
                self._update_btn.setText(LANG.about_update_check)
                self._update_btn.setEnabled(True)
                QMessageBox.warning(self, LANG.about_update_title,
                                    LANG.format("about_update_error", error=result["error"]))
                return

            if not result["has_new"]:
                self._update_btn.setText(LANG.about_update_check)
                self._update_btn.setEnabled(True)
                QMessageBox.information(self, LANG.about_update_title,
                                        LANG.about_update_latest)
                return

            # 有新版本
            reply = QMessageBox.question(
                self, LANG.about_update_title,
                LANG.format("about_update_confirm", version=result["latest"]),
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
            )
            if reply != QMessageBox.StandardButton.Yes:
                self._update_btn.setText(LANG.about_update_check)
                self._update_btn.setEnabled(True)
                return

            # 下载
            dl = ZipDownloader(result["download_url"], tempfile.gettempdir(), self)
            progress = QProgressDialog(LANG.about_update_downloading,
                                       None, 0, 0, self)
            progress.setWindowTitle(LANG.about_update_title)
            progress.setMinimumDuration(0)
            progress.setValue(0)
            self._update_btn.setText(LANG.about_update_downloading)
            self._update_btn.setEnabled(False)

            def on_progress(recv, total):
                progress.setMaximum(total)
                progress.setValue(recv)

            def on_download(err, path):
                progress.close()
                if err:
                    self._update_btn.setText(LANG.about_update_check)
                    self._update_btn.setEnabled(True)
                    QMessageBox.warning(self, LANG.about_update_title,
                                        LANG.format("about_update_error", error=err))
                    return

                # 安装
                installer = UpdateInstaller(path, self)
                installer.finished.connect(lambda e: (
                    QMessageBox.warning(self, LANG.about_update_title,
                                        LANG.format("about_update_error", error=e))
                    if e else None
                ))
                installer.install()
                # 注意：install() 调用 os._exit(0)，以下代码不会执行

            dl.progress.connect(on_progress)
            dl.finished.connect(on_download)
            dl.start()

        self._checker.finished.connect(on_check)
        self._checker.check()

    def _setup_ui(self):
        # 可滚动的外层
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        scroll.setStyleSheet("QScrollArea { border: none; background: transparent; }"
                             "QScrollBar:vertical { width: 6px; }"
                             "QScrollBar::handle:vertical { background: #CCC; border-radius: 3px; }"
                             "QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }")

        content = QWidget()
        content.setStyleSheet("background: transparent;")
        layout = QVBoxLayout(content)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.setSpacing(6)

        # ── Logo + 标题行 ──
        header = QHBoxLayout()
        header.setSpacing(23)

        logo_label = QLabel()
        logo = self._load_logo()
        if not logo.isNull():
            logo_label.setPixmap(logo.scaled(
                65, 65,
                Qt.AspectRatioMode.KeepAspectRatio,
                Qt.TransformationMode.SmoothTransformation,
            ))
        else:
            logo_label.setText(LANG.about_fallback_emoji)
            logo_label.setStyleSheet("font-size: 48px;")
        header.addWidget(logo_label)

        title_group = QVBoxLayout()
        title_group.setSpacing(2)
        title = QLabel(LANG.app_name_about)
        title.setStyleSheet(STYLE_H1)
        title_group.addWidget(title)

        ver = QLabel(LANG.format("about_version", version=__version__))
        ver.setStyleSheet(STYLE_BODY)
        ver_row = QHBoxLayout()
        ver_row.setSpacing(12)
        ver_row.addWidget(ver)

        self._update_btn = QPushButton(LANG.about_update_check)
        self._update_btn.setStyleSheet(
            "QPushButton { background: #4a9eff; color: white; border: 1px solid #4a9eff; "
            "border-radius: 4px; padding: 4px 12px; font-size: 14px; }"
            "QPushButton:hover { background: #3B82F6; }"
            "QPushButton:disabled { background: #999; border-color: #999; }"
        )
        self._update_btn.clicked.connect(self._on_check_update)
        ver_row.addWidget(self._update_btn)
        ver_row.addStretch()
        title_group.addLayout(ver_row)
        header.addLayout(title_group, stretch=1)
        layout.addLayout(header)

        # ── 分隔线 ──
        line = QFrame()
        line.setFrameShape(QFrame.Shape.HLine)
        line.setStyleSheet("color: #CCC;")
        layout.addWidget(line)

        layout.addSpacing(12)

        layout.addWidget(self._make_link_row(LANG.about_label_author, __author__, __author_url__))
        layout.addWidget(self._make_text_row(LANG.about_label_email, __email__))
        layout.addWidget(self._make_link_row(LANG.about_label_project, __url__, __url__))
        layout.addWidget(self._make_text_row(LANG.about_label_license, "AGPL-3.0"))

        # ── 开发工具 ──
        row = QWidget()
        row.setStyleSheet("background: transparent;")
        row.setFixedHeight(32)
        rl = QHBoxLayout(row)
        rl.setContentsMargins(0, 2, 0, 2)
        rl.setSpacing(8)
        rl.addWidget(self._make_label(LANG.about_label_tools))

        tool_widget = QWidget()
        tool_widget.setStyleSheet("background: transparent;")
        tl = QHBoxLayout(tool_widget)
        tl.setContentsMargins(0, 0, 0, 0)
        tl.setSpacing(6)
        first = True
        for name, url in __tools__.items():
            if not first:
                sep = QLabel(" + ")
                sep.setStyleSheet("font-size:16px; color:#000;")
                tl.addWidget(sep)
            link = QLabel(
                f'<a href="{url}" style="color:{C_LINK};text-decoration:none;font-size:16px;">{name}</a>'
            )
            link.setTextFormat(Qt.TextFormat.RichText)
            link.setOpenExternalLinks(True)
            tl.addWidget(link)
            first = False
        tl.addStretch()
        rl.addWidget(tool_widget, stretch=1)
        layout.addWidget(row)

        # ── 致谢 ──
        credit_row = QWidget()
        credit_row.setStyleSheet("background: transparent;")
        credit_row.setFixedHeight(32)
        cr = QHBoxLayout(credit_row)
        cr.setContentsMargins(0, 2, 0, 2)
        cr.setSpacing(8)
        cr.addWidget(self._make_label("致谢"))
        credit_text = QLabel(
            '<a href="https://github.com/Calinou/kenney-ui-audio" '
            f'style="color:{C_LINK};text-decoration:none;font-size:16px;">Kenney UI Audio</a>'
            '<span style="font-size:16px; color:#000;"> (CC0 1.0)</span>'
        )
        credit_text.setTextFormat(Qt.TextFormat.RichText)
        credit_text.setOpenExternalLinks(True)
        cr.addWidget(credit_text, stretch=1)
        layout.addWidget(credit_row)

        # ── 捐赠二维码（与致谢间隔两行）──
        spacer = QWidget()
        spacer.setFixedHeight(20)
        spacer.setStyleSheet("background: transparent;")
        layout.addWidget(spacer)
        donate_title = QLabel("🫶 支援我买 Token 继续改进代码")
        donate_title.setStyleSheet(STYLE_BODY)
        donate_title.setContentsMargins(0, 2, 0, 2)
        layout.addWidget(donate_title)

        qr_row = QHBoxLayout()
        qr_row.setSpacing(24)

        def _make_qr(b64: str, label: str) -> QWidget:
            from PySide6.QtSvg import QSvgRenderer
            from PySide6.QtCore import QByteArray, QRect
            from PySide6.QtGui import QPainter, QImage, QPixmap
            SZ = 220
            data = QByteArray.fromBase64(b64.encode())
            renderer = QSvgRenderer(data)

            ds = renderer.defaultSize()
            if ds.isValid() and ds.width() > 0:
                src = QImage(ds, QImage.Format.Format_ARGB32)
                src.fill(0xFFFFFFFF)
                p = QPainter(src)
                renderer.render(p)
                p.end()

                # 自动裁剪白边
                min_x, max_x, min_y, max_y = ds.width(), 0, ds.height(), 0
                for y in range(ds.height()):
                    for x in range(ds.width()):
                        c = src.pixelColor(x, y)
                        if c.alpha() > 128 and (c.red() < 250 or c.green() < 250 or c.blue() < 250):
                            if x < min_x: min_x = x
                            if x > max_x: max_x = x
                            if y < min_y: min_y = y
                            if y > max_y: max_y = y
                if max_x > min_x and max_y > min_y:
                    cropped = src.copy(QRect(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
                else:
                    cropped = src
                pix = QPixmap.fromImage(cropped.scaled(
                    SZ, SZ, Qt.AspectRatioMode.IgnoreAspectRatio,
                    Qt.TransformationMode.SmoothTransformation))
            else:
                pix = QPixmap(SZ, SZ)
                pix.fill(Qt.GlobalColor.white)
            box = QWidget()
            box.setStyleSheet("background:transparent;")
            vl = QVBoxLayout(box)
            vl.setContentsMargins(0, 0, 0, 0)
            vl.setSpacing(4)
            lbl = QLabel(label)
            lbl.setStyleSheet(STYLE_BODY)
            lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
            vl.addWidget(lbl)
            img = QLabel()
            img.setPixmap(pix)
            img.setFixedSize(SZ, SZ)
            img.setAlignment(Qt.AlignmentFlag.AlignCenter)
            vl.addWidget(img)
            return box

        qr_row.addWidget(_make_qr(WECHAT_QR, "微信"))
        qr_row.addWidget(_make_qr(ALIPAY_QR, "支付宝"))
        qr_row.addStretch()
        layout.addLayout(qr_row)

        layout.addStretch()

        scroll.setWidget(content)
        scroll_layout = QVBoxLayout(self)
        scroll_layout.setContentsMargins(0, 0, 0, 0)
        scroll_layout.addWidget(scroll)
