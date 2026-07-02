"""pynput 全局钩子线程安全桥接"""

from PySide6.QtCore import QObject, Signal


class KeyEventBridge(QObject):
    """将 pynput 的按键事件通过 Qt Signal 安全转发到主线程"""

    key_pressed = Signal(str)
    key_released = Signal(str)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.listener = None

    def start(self):
        """启动 pynput 键盘监听（守护线程，不阻止进程退出）"""
        from pynput import keyboard
        self.listener = keyboard.Listener(
            on_press=lambda k: self.key_pressed.emit(str(k)),
            on_release=lambda k: self.key_released.emit(str(k)),
        )
        self.listener.daemon = True  # 守护线程：进程退出时不等待
        self.listener.start()

    def stop(self):
        if self.listener:
            self.listener.stop()
            self.listener = None


class MouseEventBridge(QObject):
    """鼠标全局钩子"""

    moved = Signal(int, int)
    pressed = Signal(str, int, int)   # button_name, x, y
    released = Signal(str, int, int)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.listener = None

    def start(self):
        from pynput import mouse
        def on_move(x, y):
            self.moved.emit(int(x), int(y))
        def on_click(x, y, button, pressed):
            name = str(button).replace("Button.", "")
            if pressed:
                self.pressed.emit(name, int(x), int(y))
            else:
                self.released.emit(name, int(x), int(y))
        self.listener = mouse.Listener(on_move=on_move, on_click=on_click)
        self.listener.daemon = True
        self.listener.start()

    def stop(self):
        if self.listener:
            self.listener.stop()
            self.listener = None
