"""透明全屏覆盖层 — 纯文本 + 双色"""

from PySide6.QtWidgets import QWidget, QApplication
from PySide6.QtCore import Qt
from PySide6.QtGui import QPainter, QColor, QFont, QPen, QFontMetrics
import re

from curseen.key_display.controller import NotificationPlacement
from curseen.key_display import keymap


class OverlayWindow(QWidget):
    """透明全屏覆盖层"""

    KEY_PADDING = 8
    KEY_MARGIN = 6
    KEY_LINE_SPACING = 2

    def __init__(self):
        super().__init__()
        self._key_controller = None
        self._trail_controller = None
        self._setup_window()

    def _setup_window(self):
        self.setWindowFlags(Qt.WindowType.FramelessWindowHint | Qt.WindowType.WindowStaysOnTopHint | Qt.WindowType.Tool)
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)

    def set_key_controller(self, controller):
        self._key_controller = controller
        controller.items_updated.connect(self.update)
        controller.settings_changed.connect(self.update)

    def set_trail_controller(self, controller):
        self._trail_controller = controller

    def show_fullscreen(self):
        screen = QApplication.primaryScreen().geometry()
        self.setGeometry(screen)
        # Windows: 系统级鼠标穿透（参考案例同款 WS_EX_TRANSPARENT）
        import sys
        if sys.platform == "win32":
            import ctypes
            hwnd = int(self.winId())
            WS_EX_TRANSPARENT = 0x00000020
            WS_EX_LAYERED = 0x00080000
            # 在现有基础上叠加，不覆盖（保留 WS_EX_TOOLWINDOW 以隐藏任务栏图标）
            ex_style = ctypes.windll.user32.GetWindowLongW(hwnd, -20)
            ex_style |= WS_EX_TRANSPARENT | WS_EX_LAYERED
            ctypes.windll.user32.SetWindowLongW(hwnd, -20, ex_style)
        self.show()

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        if self._trail_controller:
            if self._trail_controller._trail_enabled:
                self._trail_controller.ripple_manager.draw(painter)
                eff = self._trail_controller.trail_effect
                if eff:
                    eff.draw(painter)
                # 悬浮标
                hd = self._trail_controller.hover_dot
                if hd._mode != 0:
                    x = self._trail_controller._last_x
                    y = self._trail_controller._last_y
                    hd._draw_at(painter, x, y)
        self._draw(painter)

    def _word_fun(self, word: str) -> bool:
        """判断单词是否为 fun 键（红色）"""
        info = keymap.display_info(word)
        return info is not None and info[1]  # (type, is_fun) → is_fun

    def _draw(self, painter: QPainter):
        ctrl = self._key_controller
        if not ctrl or not ctrl.items:
            return

        font = QFont()
        font.setPointSize(ctrl.font_size)
        painter.setFont(font)

        fm = QFontMetrics(font)
        win_w = self.width()
        win_h = self.height()
        padding = self.KEY_PADDING
        margin = self.KEY_MARGIN
        line_space = self.KEY_LINE_SPACING
        line_height = fm.height() + line_space
        max_width_allowed = int(win_w * ctrl.popup_width / 100.0)
        inner_w = max_width_allowed - padding * 2

        is_bottom = ctrl.placement in (NotificationPlacement.BOTTOM_LEFT, NotificationPlacement.BOTTOM_RIGHT)
        is_left = ctrl.placement in (NotificationPlacement.TOP_LEFT, NotificationPlacement.BOTTOM_LEFT)

        off_top = int(win_h * ctrl.top_offset / 100.0)
        off_bot = int(win_h * ctrl.bottom_offset / 100.0)
        off_left = int(win_w * ctrl.left_offset / 100.0)
        off_right = int(win_w * ctrl.right_offset / 100.0)

        popup_data = []
        for item in reversed(ctrl.items):
            raw_text = item.display_text
            if not raw_text:
                continue

            # 词级换行
            words = raw_text.split()
            lines = []
            cur_words = []
            cur_w = 0
            for word in words:
                rw = fm.horizontalAdvance(word)
                sep = fm.horizontalAdvance(" ") if cur_words else 0
                if cur_w + sep + rw <= inner_w:
                    cur_words.append(word)
                    cur_w += sep + rw
                else:
                    if cur_words:
                        lines.append(" ".join(cur_words))
                    cur_words = [word]
                    cur_w = rw
            if cur_words:
                lines.append(" ".join(cur_words))

            if not lines:
                continue

            max_lw = 0
            for ln in lines:
                lw = fm.horizontalAdvance(ln)
                if lw > max_lw:
                    max_lw = lw
            box_w = min(max_lw + padding * 2, max_width_allowed)
            total_h = len(lines) * line_height + padding * 2
            popup_data.append((item, lines, box_w, total_h))

        # Y 位置
        y_pos = []
        if is_bottom:
            cy = win_h - off_bot
            for _, _, _, th in popup_data:
                cy -= th; y_pos.append(cy); cy -= margin
        else:
            cy = off_top
            for _, _, _, th in popup_data:
                y_pos.append(cy); cy += th + margin

        # 绘制
        fun_col = QColor(ctrl.fun_color) if hasattr(ctrl, 'fun_color') else QColor("#ff0000")
        txt_col = QColor(ctrl.font_color)
        sym_col = QColor(ctrl.symbol_color) if hasattr(ctrl, 'symbol_color') else QColor("#000000")

        for idx, (item, lines, box_w, total_h) in enumerate(popup_data):
            y = y_pos[idx]
            x = off_left if is_left else win_w - box_w - off_right

            alpha = int(ctrl.opacity * item.opacity * 255)
            if alpha <= 0:
                continue

            bg = QColor(ctrl.bg_color)
            bg.setAlpha(alpha)
            painter.setPen(QPen(QColor(60, 60, 60, int(180 * ctrl.opacity * item.opacity)), 1))
            painter.setBrush(bg)
            painter.drawRoundedRect(x, y, box_w, total_h, 6, 6)

            text_x = x + padding
            for li, line in enumerate(lines):
                line_y = y + padding + li * line_height + fm.ascent()
                if line.startswith(("Ctrl +", "Alt +", "Shift +", "Win +", "Menu +")):
                    self._draw_shortcut(painter, line, text_x, line_y, fm, txt_col, fun_col, sym_col, alpha)
                else:
                    self._draw_plain(painter, line, text_x, line_y, fm, txt_col, fun_col, sym_col, alpha)

    def _draw_plain(self, painter, line, x, y, fm, txt_col, fun_col, sym_col, alpha):
        """纯文字行，逐词着色；x N 用符号颜色"""
        tokens = re.split(r'( )', line)
        i = 0
        while i < len(tokens):
            tok = tokens[i]
            if tok == ' ':
                painter.setPen(self._qcol(txt_col, alpha))
                painter.drawText(x, y, tok)
                x += fm.horizontalAdvance(tok)
                i += 1
            elif tok == 'x' and i + 2 < len(tokens) and tokens[i+1] == ' ' and tokens[i+2].strip().isdigit():
                painter.setPen(self._qcol(sym_col, alpha))
                painter.drawText(x, y, tok)
                x += fm.horizontalAdvance(tok)
                painter.drawText(x, y, tokens[i+1])
                x += fm.horizontalAdvance(tokens[i+1])
                painter.drawText(x, y, tokens[i+2])
                x += fm.horizontalAdvance(tokens[i+2])
                i += 3
            else:
                ws = tok.strip()
                c = fun_col if ws and self._word_fun(ws) else txt_col
                painter.setPen(self._qcol(c, alpha))
                painter.drawText(x, y, tok)
                x += fm.horizontalAdvance(tok)
                i += 1

    def _draw_shortcut(self, painter, line, x, y, fm, txt_col, fun_col, sym_col, alpha):
        """快捷键行，各 part 独立着色；+ 用符号颜色；x N 用符号颜色"""
        parts = line.split(" + ")
        for idx, part in enumerate(parts):
            if idx > 0:
                c = self._qcol(sym_col, alpha)
                painter.setPen(c)
                painter.drawText(x, y, " + ")
                x += fm.horizontalAdvance(" + ")

            # 检查最后一个 part 是否有 " x N" 后缀
            if idx == len(parts) - 1:
                m = re.match(r'^(.+?)( x \d+)$', part)
                if m:
                    key_part = m.group(1)
                    count_part = m.group(2)
                    c = fun_col if self._word_fun(key_part) else txt_col
                    painter.setPen(self._qcol(c, alpha))
                    painter.drawText(x, y, key_part)
                    x += fm.horizontalAdvance(key_part)
                    painter.setPen(self._qcol(sym_col, alpha))
                    painter.drawText(x, y, count_part)
                    x += fm.horizontalAdvance(count_part)
                else:
                    c = fun_col if self._word_fun(part) else txt_col
                    painter.setPen(self._qcol(c, alpha))
                    painter.drawText(x, y, part)
                    x += fm.horizontalAdvance(part)
            else:
                c = fun_col if self._word_fun(part) else txt_col
                painter.setPen(self._qcol(c, alpha))
                painter.drawText(x, y, part)
                x += fm.horizontalAdvance(part)

    @staticmethod
    def _qcol(color: QColor, alpha: int) -> QColor:
        c = QColor(color)
        c.setAlpha(alpha)
        return c
