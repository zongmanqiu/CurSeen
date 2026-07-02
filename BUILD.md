# 构建与打包指南

## 环境要求

- Python 3.11.3（其他 3.10-3.12 版本也可能可用）
- Windows 10/11（64 位）
- 依赖：`pip install -r requirements.txt`

## 开发运行

```powershell
cd CurSeen
python -m curseen
```

或双击 `scripts\run.bat`（无控制台窗口） / `scripts\run_debug.bat`（显示控制台）。

---

## 生产打包（Nuitka + UPX）

### 1. 安装依赖

```powershell
pip install nuitka==4.1.3
pip install -r requirements.txt
```

### 2. 安装 UPX（强烈推荐，减小 DLL 体积 ~60%）


```powershell
Invoke-WebRequest -Uri "https://github.com/upx/upx/releases/download/v4.2.4/upx-4.2.4-win64.zip" -OutFile "E:\tmp\upx.zip"
Expand-Archive "E:\tmp\upx.zip" -DestinationPath "E:\tmp\upx" -Force
```

### 3. 执行编译

```powershell
cd CurSeen

python -m nuitka `
    --standalone `
    --assume-yes-for-downloads `
    --enable-plugin=pyside6 `
    --windows-console-mode=disable `
    --output-dir=build_v2 `
    --include-package=curseen,resources `
    --include-module=curseen.__main__ `
    curseen
```

编译耗时约 **20-40 分钟**，首次运行会下载 Dependency Walker（自动）。

### 4. 编译后清理与优化

```powershell
# 4a. 删除不必要的 Qt 图像格式插件
$DIST = "build_v2\curseen.dist"
@(
    "PySide6\qt-plugins\imageformats\qicns.dll",   # macOS
    "PySide6\qt-plugins\imageformats\qpdf.dll",      # PDF
    "PySide6\qt-plugins\imageformats\qtga.dll",      # TGA
    "PySide6\qt-plugins\imageformats\qwbmp.dll",     # WBMP
    "PySide6\qt-plugins\imageformats\qwebp.dll",     # WebP
    "PySide6\qt-plugins\imageformats\qtiff.dll",     # TIFF
    "PySide6\qt-plugins\platforms\qdirect2d.dll",    # Direct2D
    "PySide6\qt-plugins\platforms\qminimal.dll",     # 嵌入式
    "PySide6\qt-plugins\platforms\qoffscreen.dll",   # 无头
    "PySide6\qt-plugins\styles\qmodernwindowsstyle.dll",
    "PySide6\qt-plugins\tls\qcertonlybackend.dll",
    "PySide6\qt-plugins\tls\qopensslbackend.dll"
    # qschannelbackend.dll 保留（Windows 原生 TLS，不依赖 OpenSSL）
) | ForEach-Object {
    $p = Join-Path $DIST $_
    if (Test-Path $p) { Remove-Item $p -Force }
}

# 4b. 删除 __pycache__
Get-ChildItem $DIST -Directory -Filter "__pycache__" -Recurse | ForEach-Object { Remove-Item $_.FullName -Recurse -Force }

# 4c. 复制资源文件（音效和拖尾特效）
New-Item -ItemType Directory -Path "$DIST\resources\audio" -Force >$null
New-Item -ItemType Directory -Path "$DIST\resources\effects" -Force >$null
Copy-Item "resources\audio\*.wav" "$DIST\resources\audio\" -Force
Copy-Item "resources\effects\*.py" "$DIST\resources\effects\" -Force

# 4d. 复制 SSL 库文件（Nuitka 自带 libcrypto-1_1.dll 但遗漏 libssl-1_1.dll）
#    缺少 libssl 会导致版本检查时输出 "TLS initialization failed"
#    查找 Python 安装目录下的 DLLs\libssl-1_1.dll 并复制
$ssl_src = Join-Path (Split-Path (Get-Command python).Source -Parent) "..\DLLs\libssl-1_1.dll"
if (Test-Path $ssl_src) {
    Copy-Item $ssl_src "$DIST\" -Force
    Write-Host "Copied libssl-1_1.dll"
} else {
    Write-Warning "libssl-1_1.dll not found — update checks may fail"
}
```

### 5. UPX 压缩 DLL

必须跳过的文件：
- `PySide6\qt-plugins\` 下的所有 DLL（UPX 会破坏 Qt 插件，导致报 `no Qt platform plugin could be initialized`）
- `*.pyd` 文件（Python 扩展模块，UPX 压缩后可能加载失败）

```powershell
$DIST = "build_v2\curseen.dist"
$UPX = "E:\tmp\upx\upx-4.2.4-win64\upx.exe"

# 逐个压缩大型 DLL（显式列表比通配符更可靠）
@("qt6core.dll","qt6gui.dll","qt6widgets.dll","python311.dll",
  "libcrypto-1_1.dll","qt6network.dll","qt6multimedia.dll","qt6svg.dll"
) | ForEach-Object {
    & $UPX --best --force (Join-Path $DIST $_)
}
```

### 6. 添加程序图标

```powershell
# 生成 ICO（从 SVG 渲染并编码为 PNG-ICO）
python -c "
from PySide6.QtSvg import QSvgRenderer
from PySide6.QtGui import QImage, QPainter
from PySide6.QtCore import QByteArray, QBuffer
import struct, base64, sys
sys.path.insert(0, '.')  # 确保能找到 resources.icons

from resources.icons import LOGO_SVG

renderer = QSvgRenderer(QByteArray(base64.b64decode(LOGO_SVG)))
img = QImage(256, 256, QImage.Format.Format_ARGB32)
img.fill(0)
painter = QPainter(img)
renderer.render(painter)
painter.end()

ba = QByteArray()
buf = QBuffer(ba)
buf.open(QBuffer.OpenModeFlag.WriteOnly)
img.save(buf, 'PNG')
buf.close()
png = bytes(ba)

ico = struct.pack('<HHH', 0, 1, 1) + struct.pack('<BBBBHHII', 0,0,0,0, 1, 32, len(png), 22) + png
with open('build_v2/curseen.ico', 'wb') as f: f.write(ico)
"

# 注入到 exe（使用 rcedit）
Invoke-WebRequest -Uri "https://github.com/electron/rcedit/releases/download/v2.0.0/rcedit-x64.exe" -OutFile "E:\tmp\rcedit.exe"
Start-Process -FilePath "E:\tmp\rcedit.exe" -ArgumentList "`"$DIST\curseen.exe`"","--set-icon","`"build_v2\curseen.ico`"" -Wait -NoNewWindow
```

### 7. 打包分发

```powershell
Compress-Archive -Path "$DIST\*" -DestinationPath "build_v2\CurSeen_$(python -c "import curseen; print(curseen.__version__)").zip" -CompressionLevel Optimal
```

产物位置：`build_v2\CurSeen_{version}.zip`（约 **27 MB**，解压后约 **48 MB**）

最后可删除编译中间文件，仅保留 ZIP：

```powershell
Remove-Item "build_v2\curseen.build" -Recurse -Force
Remove-Item "build_v2\curseen.dist" -Recurse -Force
Remove-Item "build_v2\curseen.ico" -Force
```

---

## 发布版文件结构

解压 `CurSeen_{version}.zip` 后：

```
CurSeen_{version}/
├── curseen.exe              # 主程序（已注入应用图标）
├── python311.dll            # Python 运行时（UPX 压缩）
├── qt6core.dll              # Qt6 核心（UPX 压缩 ~3.4 MB）
├── qt6gui.dll               # Qt6 图形（UPX 压缩 ~4.0 MB）
├── qt6widgets.dll           # Qt6 控件（UPX 压缩 ~2.8 MB）
├── qt6multimedia.dll        # Qt6 多媒体（UPX 压缩）
├── qt6network.dll           # Qt6 网络（UPX 压缩）
├── qt6svg.dll               # Qt6 SVG 渲染
├── libcrypto-1_1.dll        # OpenSSL 加密（UPX 压缩）
├── libssl-1_1.dll           # OpenSSL SSL（手动补入）
├── *.pyd                    # Python 扩展模块
├── PySide6/                 # Qt6 Python 绑定
│   ├── QtCore.pyd / QtGui.pyd / QtWidgets.pyd ...
│   └── qt-plugins/          # Qt 插件（保留，**未** UPX 压缩）
│       ├── platforms/qwindows.dll
│       ├── imageformats/qjpeg.dll, qgif.dll, qico.dll, qsvg.dll
│       └── tls/qschannelbackend.dll
├── shiboken6/               # Python/C++ 桥接
└── resources/
    ├── audio/*.wav          # 音效文件（Kenney UI Audio, CC0 1.0）
    └── effects/*.py         # 拖尾特效插件
```

---

## 注意事项

1. **Qt 插件禁止 UPX 压缩** — 否则报 `no Qt platform plugin could be initialized`
2. **UPX 版本** — 推荐 4.2.4，新版可能引入兼容问题
3. **UPX 路径** — 需确保 `E:\tmp\upx\upx-4.2.4-win64\upx.exe` 可执行，若被清理需重新下载
4. **libssl-1_1.dll** — Nuitka 不自带此文件，缺少会导致版本检查失败（SSL 错误），需从 Python 安装目录手动复制
5. **QSchannel** — 保留 `qschannelbackend.dll`（Windows 原生 TLS），删除 `qcertonlybackend.dll` 和 `qopensslbackend.dll`
6. **SSL 验证** — `checker.py` 使用 `urllib` + 后台线程而非 `QNetworkAccessManager`，避免 Qt SSL 配置问题
7. **更新机制** — 版本更新由 `curseen` 下载 ZIP → 生成 `curseen_update.bat` → 退出；bat 负责解压、复制、重启
8. **杀软误报** — UPX 压缩的 exe/DLL 可能被 Defender 或第三方杀软报毒，正常现象
9. **Python 版本** — 必须与编译时使用的 Python 版本一致（目前 3.11）
10. **SVG 图标** — 源代码中 `resources/icons/__init__.py` 内嵌 SVG base64，改 Logo 后需重新编译
