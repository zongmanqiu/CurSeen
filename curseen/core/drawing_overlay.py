"""绘图覆盖层 — 全屏透明标画板 / 白色白板 + 形状工具"""

from PySide6.QtCore import Qt, QPointF, QRectF, QRect
from PySide6.QtGui import QPainter, QColor, QPen, QBrush, QPainterPath, QPolygonF
from PySide6.QtWidgets import QWidget, QApplication, QInputDialog
import math


class DrawingOverlay(QWidget):
    """半透明/白色全屏绘图覆盖层，支持画笔+形状+文字"""

    def __init__(self):
        super().__init__()
        self._history = None
        self._current_path = None
        self._current_color = QColor("#FF0000")
        self._current_width = 4
        self._current_style = "solid"
        self._mode = "off"        # "off" | "screen" | "whiteboard"
        self._tool = "pen"        # pen / line / arrow / rect / ellipse / text / eraser
        self._shape_start = None
        self._shape_preview = None
        self._arrow_head = None
        self._setup_window()

    def _setup_window(self):
        self.setWindowFlags(
            Qt.WindowType.FramelessWindowHint |
            Qt.WindowType.WindowStaysOnTopHint |
            Qt.WindowType.Tool
        )
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
        self.hide()

    def set_history(self, history):
        self._history = history

    def start_drawing(self, mode: str = "screen"):
        import time
        self._mode = mode
        self._notify_text = "屏幕标注模式" if mode == "screen" else "白板模式"
        self._notify_time = time.perf_counter()
        screen = QApplication.primaryScreen().geometry()
        self.setGeometry(screen)
        if not self.isVisible():
            self.show()
            self.setFocus()
            self.raise_()
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, False)
        self.update()

    def stop_drawing(self):
        import time
        self._mode = "off"
        self._notify_text = "已退出绘图"
        self._notify_time = time.perf_counter()
        self._current_path = None
        self._shape_start = None
        self._shape_preview = None
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents, True)
        self.update()
        from PySide6.QtCore import QTimer
        QTimer.singleShot(2000, self._clear_notify)

    def _clear_notify(self):
        self._notify_text = ""
        self.update()

    def set_pen(self, color: QColor, width: int, style: str = "solid"):
        self._current_color = color
        self._current_width = width
        self._current_style = style

    def set_tool(self, tool: str):
        self._tool = tool
        if tool == "eraser":
            self.setCursor(Qt.CursorShape.CrossCursor)
        else:
            self.setCursor(Qt.CursorShape.ArrowCursor)

    def begin_stroke(self, x: int, y: int):
        if self._mode == "off":
            return
        if self._tool in ("pen", "eraser"):
            self._current_path = QPainterPath()
            self._current_path.moveTo(QPointF(x, y))
        elif self._tool == "text":
            text, ok = QInputDialog.getText(self, "文字", "输入文字:")
            if ok and text and self._history:
                path = QPainterPath()
                font = self.font()
                font.setPixelSize(self._current_width * 4)
                path.addText(QPointF(x, y), font, text)
                self._history.add(path, self._current_color.name(QColor.NameFormat.HexArgb),
                                  self._current_width, self._current_style)
                self.update()
        elif self._tool in ("line", "arrow", "rect", "ellipse"):
            self._shape_start = QPointF(x, y)
            self._shape_preview = None

    def extend_stroke(self, x: int, y: int):
        if self._current_path and self._tool in ("pen", "eraser"):
            self._current_path.lineTo(QPointF(x, y))
            self.update()
        elif self._tool in ("line", "arrow", "rect", "ellipse") and self._shape_start:
            self._shape_preview = QPointF(x, y)
            self._completed_shape = None  # still previewing
            self.update()

    def end_stroke(self):
        if self._current_path and self._tool in ("pen", "eraser") and self._history:
            if self._tool == "eraser":
                # 橡皮擦：移除与擦除区域相交的整条笔迹
                eraser_rect = self._current_path.boundingRect()
                self._history.erase_by_rect(eraser_rect)
            else:
                self._history.add(
                    self._current_path,
                    self._current_color.name(QColor.NameFormat.HexArgb),
                    self._current_width,
                    self._current_style,
                )
        elif self._tool in ("line", "arrow", "rect", "ellipse") and self._shape_start and self._shape_preview:
            shape_path = self._make_shape(self._shape_start, self._shape_preview, self._tool)
            if shape_path and self._history:
                # 箭头：线用轮廓模式，再单独填充箭头三角形
                is_shape = False
                self._history.add(
                    shape_path, self._current_color.name(QColor.NameFormat.HexArgb),
                    self._current_width, "solid", is_shape=is_shape,
                )
                if self._arrow_head and self._tool == "arrow":
                    # 把箭头三角形作为封闭多边形存入历史
                    ah_path = QPainterPath()
                    ah_path.addPolygon(self._arrow_head)
                    ah_path.closeSubpath()
                    self._history.add(
                        ah_path, self._current_color.name(QColor.NameFormat.HexArgb),
                        max(self._current_width, 1), "solid", is_shape=True,
                    )
        self._current_path = None
        self._shape_start = None
        self._shape_preview = None
        self.update()

    def _make_shape(self, p1: QPointF, p2: QPointF, tool: str) -> QPainterPath | None:
        path = QPainterPath()
        if tool == "line":
            path.moveTo(p1)
            path.lineTo(p2)
        elif tool == "arrow":
            # 返回路径为线（轮廓），箭头三角形单独存储供填充
            w = self._current_width
            path.moveTo(p1)
            path.lineTo(p2)
            dx, dy = p2.x() - p1.x(), p2.y() - p1.y()
            length = math.hypot(dx, dy)
            self._arrow_head = None
            if length > 5:
                ux, uy = dx / length, dy / length
                nx, ny = -uy, ux
                head_len = max(15.0, w * 4)
                head_half = max(10.0, w * 3)
                head_len = min(head_len, length * 0.45)
                tip = QPointF(p2)
                left = QPointF(p2.x() - ux * head_len + nx * head_half,
                               p2.y() - uy * head_len + ny * head_half)
                right = QPointF(p2.x() - ux * head_len - nx * head_half,
                                p2.y() - uy * head_len - ny * head_half)
                self._arrow_head = QPolygonF([tip, left, right])
            # 注意：caller 需在 _draw_path（NoBrush）后再绘制 self._arrow_head
        elif tool == "rect":
            r = QRectF(p1, p2).normalized()
            path.addRect(r)
        elif tool == "ellipse":
            r = QRectF(p1, p2).normalized()
            path.addEllipse(r)
        return path

    def undo(self):
        if self._history:
            self._history.undo()
            self.update()
            return True
        return False

    def clear_all(self):
        if self._history:
            self._history.clear()
            self.update()

    def keyPressEvent(self, event):
        if event.key() == Qt.Key.Key_Escape:
            self.stop_drawing()

    def paintEvent(self, event):
        if self._mode == "off":
            painter = QPainter(self)
            if self._notify_text:
                self._draw_notify(painter)
            return
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        if self._mode == "whiteboard":
            painter.fillRect(self.rect(), QColor(255, 255, 255))  # 纯白不透明
        else:
            painter.fillRect(self.rect(), QColor(0, 0, 0, 1))

        self._draw_notify(painter)

        # 历史笔迹
        if self._history:
            for s in self._history.strokes:
                self._draw_path(painter, s["path"], s["color"], s["width"], s["style"],
                                s.get("is_shape", False))

        # 当前笔迹
        if self._current_path:
            if self._tool == "eraser":
                erase_color = QColor(255, 255, 255) if self._mode == "whiteboard" else QColor(40, 40, 40)
                self._draw_path(painter, self._current_path, erase_color.name(),
                                max(self._current_width, 20), "solid")
            else:
                self._draw_path(painter, self._current_path,
                                self._current_color.name(QColor.NameFormat.HexArgb), self._current_width, self._current_style)

        if self._tool in ("line", "arrow", "rect", "ellipse") and self._shape_start and self._shape_preview:
            preview = self._make_shape(self._shape_start, self._shape_preview, self._tool)
            if preview:
                is_shape = False
                self._draw_path(painter, preview, self._current_color.name(QColor.NameFormat.HexArgb),
                                self._current_width, "solid", is_shape=is_shape)
                if self._arrow_head and self._tool == "arrow":
                    ah_path = QPainterPath()
                    ah_path.addPolygon(self._arrow_head)
                    ah_path.closeSubpath()
                    self._draw_path(painter, ah_path, self._current_color.name(QColor.NameFormat.HexArgb),
                                    max(self._current_width, 1), "solid", is_shape=True)

        painter.end()

    def _draw_notify(self, painter: QPainter):
        import time
        if not self._notify_text:
            return
        elapsed = time.perf_counter() - self._notify_time
        if elapsed > 2.0:
            return
        alpha = int(255 * max(0.0, 1.0 - elapsed / 2.0))
        font = painter.font()
        font.setPointSize(28)
        font.setBold(True)
        painter.setFont(font)
        # 描边
        pen = QPen(QColor(0, 0, 0, alpha), 1)
        painter.setPen(pen)
        painter.setBrush(QColor(255, 255, 255, alpha))
        path = QPainterPath()
        fm = painter.fontMetrics()
        tw = fm.horizontalAdvance(self._notify_text)
        x = (self.width() - tw) // 2
        y = self.height() // 2 + fm.ascent() // 2
        path.addText(x, y, font, self._notify_text)
        painter.drawPath(path)

    def _draw_path(self, painter: QPainter, path, color: str, width: int, style: str, is_shape: bool = False):
        qc = QColor(color)
        pen = QPen(qc, width)
        if style == "dashed":
            pen.setStyle(Qt.PenStyle.DashLine)
        elif style == "dotted":
            pen.setStyle(Qt.PenStyle.DotLine)
        elif style == "dash_dot":
            pen.setStyle(Qt.PenStyle.DashDotLine)
        else:
            pen.setStyle(Qt.PenStyle.SolidLine)
        pen.setCapStyle(Qt.PenCapStyle.RoundCap)
        pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
        painter.setPen(pen)
        painter.setBrush(QBrush(qc) if is_shape else Qt.BrushStyle.NoBrush)
        if isinstance(path, QPainterPath):
            painter.drawPath(path)

    def mousePressEvent(self, event):
        if self._mode == "off" or event.button() != Qt.MouseButton.LeftButton:
            return
        self.begin_stroke(int(event.position().x()), int(event.position().y()))

    def mouseMoveEvent(self, event):
        if self._mode == "off":
            return
        self.extend_stroke(int(event.position().x()), int(event.position().y()))

    def mouseReleaseEvent(self, event):
        if self._mode == "off" or event.button() != Qt.MouseButton.LeftButton:
            return
        self.end_stroke()
