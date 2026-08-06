## macOS 安装说明

请根据 Mac 类型下载对应的 DMG：

- **Apple Silicon**（M1 及后续机型）：`Lumen-Navi-v*-arm64.dmg`
- **Intel Mac**：`Lumen-Navi-v*-x64.dmg`

双击 DMG，将 **Lumen Navi** 拖入 Applications。

### 首次打开

发布构建使用 **Developer ID 签名并公证**。如果 macOS 仍阻止启动，请先确认安装包来自本仓库发布页且校验值一致，不要绕过来源不明应用的 Gatekeeper 提示。

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

### 说明

- 应用内嵌 `Lumen Cua.app`（屏幕权限与捕获）和 `lumen-daemon`（策略、存储、OCR、ASR）。
- 数据默认：`~/Library/Application Support/LumenNavi/`
- **听写/热键注入** 请用独立产品 [Lumen ASR](https://github.com/fakechris/lumen-asr)，与本仓库无关。
