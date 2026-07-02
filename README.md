# CurSeen / 可寻

**键鼠痕迹呈现工具**：鼠标特效 · 按键显示 · 屏幕书写

- **版本**：1.0.0
- **作者**：邱宗满（qiuzongman@foxmail.com）
- **项目地址**：<https://gitee.com/qiuzongman/curseen>
- **许可**：AGPL-3.0
- **开发工具**：[Reasonix](https://reasonix.io/) · [DeepSeek](https://www.deepseek.com/)

## 功能

| 模块 | 说明 |
|---|---|
| 🖱 **鼠标特效** | 悬浮标（实心/空心圆）、点击声、点击波纹、多种拖尾特效（像素尘埃、律动粒子、流光飘带、蛛丝缠网、辉光圆影） |
| ⌨️ **按键显示** | 实时显示键盘按键，104 键完整映射，修饰键组合高亮，自动换行淡出 |
| ✏️ **屏幕书写** | 屏幕标注与白板模式，多支画笔，形状工具（直线、箭头、矩形、椭圆），橡皮擦 |
| ⚙ **全局设置** | 功能模块独立开关，面板快捷键，一键恢复默认 |

## 快速开始

```powershell
# 激活虚拟环境
.\venv\Scripts\Activate.ps1

# 运行
python -m curseen
```

或双击 `scripts\run.bat`（无控制台窗口）。

## 项目结构

```
CurSeen/
├── curseen/               # 主程序包
│   ├── core/              # 核心逻辑（窗口、覆盖层、桥接、配置）
│   ├── trail/             # 鼠标拖尾模块
│   ├── key_display/       # 按键显示模块
│   ├── drawing/           # 屏幕书写模块
│   ├── about/             # 关于页面
│   └── update/            # 自动更新
├── resources/             # 资源文件
│   ├── audio/             # 音效
│   ├── effects/           # 拖尾特效插件
│   └── icons/             # 图标 base64
├── scripts/               # 运行/停止脚本
├── BUILD.md               # 构建打包指南
├── HISTORY.md             # 版本历史
└── SOURCES.md             # 资源来源说明
```

## 打包

详见 [BUILD.md](BUILD.md)。

## 系统要求

- Windows 10/11 64 位
- Python 3.11+（开发）
- 无需安装，解压即用（发行版）
