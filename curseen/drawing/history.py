"""绘图历史 — 撤销/重做栈"""

from PySide6.QtGui import QPainterPath


class DrawingHistory:
    """笔迹历史：每条记录是一个 QPainterPath + 笔参数"""

    def __init__(self):
        self._strokes: list[dict] = []
        self._undo_stack: list[dict] = []

    def add(self, path: QPainterPath, color: str, width: int, style: str, is_shape: bool = False):
        self._strokes.append({
            "path": path, "color": color, "width": width, "style": style, "is_shape": is_shape,
        })
        self._undo_stack.clear()  # 新笔迹清空 redo

    def undo(self) -> bool:
        if not self._strokes:
            return False
        self._undo_stack.append(self._strokes.pop())
        return True

    def erase(self, eraser_path: 'QPainterPath'):
        """擦除：从所有笔迹中减去擦除路径"""
        new_strokes = []
        for s in self._strokes:
            remaining = s["path"].subtracted(eraser_path)
            if not remaining.isEmpty():
                s["path"] = remaining
                new_strokes.append(s)
        self._strokes = new_strokes

    def erase_by_rect(self, rect: 'QRectF'):
        """按矩形区域删除完全相交的笔迹"""
        self._strokes = [s for s in self._strokes
                         if not s["path"].boundingRect().intersects(rect)]

    def redo(self) -> bool:
        if not self._undo_stack:
            return False
        self._strokes.append(self._undo_stack.pop())
        return True

    def clear(self):
        self._strokes.clear()
        self._undo_stack.clear()

    @property
    def strokes(self) -> list[dict]:
        return self._strokes
