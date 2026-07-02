"""ZIP 下载（带进度信号）"""

import os
from PySide6.QtCore import QObject, Signal
from PySide6.QtNetwork import QNetworkAccessManager, QNetworkRequest
from PySide6.QtCore import QUrl


class ZipDownloader(QObject):
    """将 ZIP 下载到临时路径，逐字节写入"""

    progress = Signal(int, int)  # current, total
    finished = Signal(str, str)  # error_or_none, local_path

    def __init__(self, url: str, save_dir: str, parent=None):
        super().__init__(parent)
        self._url = url
        self._save_dir = save_dir
        self._mgr = QNetworkAccessManager(self)
        self._mgr.finished.connect(self._on_done)
        self._local_path = ""
        self._file = None

    def start(self):
        os.makedirs(self._save_dir, exist_ok=True)
        self._local_path = os.path.join(self._save_dir, "update.zip")
        req = QNetworkRequest(QUrl(self._url))
        req.setHeader(QNetworkRequest.KnownHeaders.UserAgentHeader, "CurSeen/1.0")
        self._reply = self._mgr.get(req)
        self._reply.downloadProgress.connect(self._on_progress)

    def _on_progress(self, recv, total):
        self.progress.emit(recv, total)

    def _on_done(self, reply):
        if reply.error() != reply.NetworkError.NoError:
            err = reply.errorString()
            self.finished.emit(err, "")
            reply.deleteLater()
            return
        try:
            data = bytes(reply.readAll())
            with open(self._local_path, "wb") as f:
                f.write(data)
            self.finished.emit(None, self._local_path)
        except Exception as e:
            self.finished.emit(str(e), "")
        reply.deleteLater()
