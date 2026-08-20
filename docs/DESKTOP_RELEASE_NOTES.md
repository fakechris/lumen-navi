# Lumen Navi v0.2.0

Local-first continuous context: smart screenshots, time tracking, 15-minute History cards, on-device OCR search, optional Roast/Chat.

![Overview](https://raw.githubusercontent.com/fakechris/lumen-navi/v0.2.0/docs/images/overview.jpg)

![Time](https://raw.githubusercontent.com/fakechris/lumen-navi/v0.2.0/docs/images/time.jpg)

## What's new since v0.1.0

- **Time tab** — 15-minute History cards with LLM narrative, app marks, day timeline. Idle and lock time are excluded.
- **Observe** — focus / visual-change screenshots; a 2-minute static-screen **liveness** frame (last JPEG per display, not OCR'd, not on the timeline).
- **AI tab** — Roast + Chat over the day's evidence (optional local / OpenAI-compat / Anthropic). Conservative CUA-replay chips on long stretches.
- **Windows** — unsigned x64 NSIS installer (see gaps below).
- **AX + scene** — accessibility text for recall; scene rules on the Time dashboard.
- Capture health, daemon supervisor, mic device picker, Chrome Observe extension.

Full changelog: [`CHANGELOG.md`](../CHANGELOG.md). Capture policy: [`OBSERVE_CAPTURE.md`](OBSERVE_CAPTURE.md).

---

## macOS 安装说明

请根据 Mac 类型下载对应的 DMG：

- **Apple Silicon**（M1 及后续机型）：`Lumen-Navi-v*-arm64.dmg`
- **Intel Mac**：`Lumen-Navi-v*-x64.dmg`

双击 DMG，将 **Lumen Navi** 拖入 Applications。

### 首次打开（未公证）

GitHub 上的 macOS 包 **没有 Apple 公证**。拖进 Applications 后，系统会提示「无法打开，因为无法验证开发者」或「已损坏」。这是隔离标记，不是包坏了。

终端里清掉隔离（把路径按你的安装位置改）：

```bash
xattr -d com.apple.quarantine "/Applications/Lumen Navi.app"
```

还拦着，就清整棵树：

```bash
xattr -cr "/Applications/Lumen Navi.app"
```

然后 **右键 → 打开**，不要双击。Sequoia：系统设置 → 隐私与安全性 → **仍要打开**。

Navi 第一次运行会把 **Lumen Cua** 装到 `/Applications/Lumen Cua.app`。如果屏幕录制权限弹不出来，对 Cua 再做一次：

```bash
xattr -d com.apple.quarantine "/Applications/Lumen Cua.app"
```

请只从本仓库的 [GitHub Releases](https://github.com/fakechris/lumen-navi/releases) 下载，并用 `SHA256SUMS.txt` 校验：

```bash
# Apple Silicon
grep 'arm64\.dmg$' SHA256SUMS.txt | shasum -a 256 --check

# Intel
grep 'x64\.dmg$' SHA256SUMS.txt | shasum -a 256 --check
```

### 权限

按应用首次引导授予：

| 权限 | 用途 |
|------|------|
| 屏幕录制（Lumen Cua） | 截图 Observe |
| 麦克风 | 音频 chunk |
| 语音识别 | 本地转写（Observe ASR） |
| 辅助功能 | 划词弹窗 / AX 文本 |

### 说明

- 应用内嵌 `Lumen Cua.app`（屏幕权限与捕获）和 `lumen-daemon`（策略、存储、OCR、ASR）。
- **Lumen Cua** 安装到 `/Applications/Lumen Cua.app`，无窗口。请在 Navi 里点「请求屏幕录制」，不要双击 Cua。
- 数据默认：`~/Library/Application Support/LumenNavi/`
- **听写/热键注入** 请用独立产品 [Lumen ASR](https://github.com/fakechris/lumen-asr)，与本仓库无关。

---

## Windows 安装说明

下载 `Lumen-Navi-v*-windows-x64-setup.exe`（Windows 10/11 x64）。

### 首次打开

安装包 **未做代码签名**，SmartScreen 会提示「已保护你的电脑」：点击「更多信息 → 仍要运行」。
请只从本仓库的 [GitHub Releases](https://github.com/fakechris/lumen-navi/releases) 下载，并校验：

```powershell
Get-FileHash .\Lumen-Navi-v*-windows-x64-setup.exe -Algorithm SHA256
```

与 `SHA256SUMS.txt` 中的对应行比对。

安装为 **当前用户** 模式（不需要管理员权限），装到 `%LOCALAPPDATA%\Lumen Navi`。
缺少 WebView2 运行时时安装器会自动静默下载。

### 权限

| 权限 | 用途 | 说明 |
|------|------|------|
| 屏幕截取 | 截图 Observe | Windows 桌面程序无需授权 |
| 麦克风 | 音频 chunk | 「设置 → 隐私和安全性 → 麦克风」需允许桌面应用 |

OCR 走系统 `Windows.Media.Ocr`，需要在「设置 → 时间和语言 → 语言和区域」为对应语言
安装可选功能中的 **光学字符识别 (OCR)** 语言包。

### 与 macOS 版的差异

- 无「macOS Speech」引擎：ASR 请选择本地 SenseVoice / Whisper，或配置云端引擎。
- 划词弹窗（选中取词 + LLM）暂未实现，UI 中会标注为不支持。
- 数据默认：`%LOCALAPPDATA%\LumenNavi\`，模型共享目录 `%LOCALAPPDATA%\Lumen\models\`。
- 已知缺口：[`WINDOWS_PORT_STATUS.md`](WINDOWS_PORT_STATUS.md)。
