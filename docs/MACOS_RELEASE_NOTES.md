# Lumen Navi v0.2.0 (macOS)

Product notes and Windows install: [`DESKTOP_RELEASE_NOTES.md`](DESKTOP_RELEASE_NOTES.md).

## macOS 安装说明

请根据 Mac 类型下载对应的 DMG：

- **Apple Silicon**（M1 及后续机型）：`Lumen-Navi-v*-arm64.dmg`
- **Intel Mac**：`Lumen-Navi-v*-x64.dmg`

双击 DMG，将 **Lumen Navi** 拖入 Applications。

### 首次打开（未公证）

GitHub 上的 macOS 包 **没有 Apple 公证**。拖进 Applications 后若提示无法验证开发者或已损坏，先清隔离：

```bash
xattr -d com.apple.quarantine "/Applications/Lumen Navi.app"
```

不行再用 `xattr -cr "/Applications/Lumen Navi.app"`，然后 **右键 → 打开**。Cua 装好后若屏幕录制弹不出来：

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

### 说明

- 应用内嵌 `Lumen Cua.app`（屏幕权限与捕获）和 `lumen-daemon`（策略、存储、OCR、ASR）。
- **Lumen Cua** 安装到 `/Applications/Lumen Cua.app`，无窗口；系统设置 → 屏幕录制中显示 **CUA 光标** 图标（`AppIcon.icns`）。请在 Navi 里点「请求屏幕录制」，不要双击 Cua。
- 数据默认：`~/Library/Application Support/LumenNavi/`
- **听写/热键注入** 请用独立产品 [Lumen ASR](https://github.com/fakechris/lumen-asr)，与本仓库无关。
