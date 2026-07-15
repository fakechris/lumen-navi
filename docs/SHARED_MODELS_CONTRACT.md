# Lumen 共享 ASR 模型契约

版本：1.0
更新日期：2026-07-15
契约源：`lumen-asr/docs/SHARED_MODELS_CONTRACT.md`
同步副本：`lumen-navi/docs/SHARED_MODELS_CONTRACT.md`

本文定义 Lumen 应用簇发现、选择和安装本地 ASR 模型时必须共同遵守的行为。文中的“必须”是兼容性要求；修改契约时，两个仓库的文档与契约测试必须在同一批变更中同步。

## 1. 共享根目录

根目录解析优先级：

1. 应用显式配置的 `asr.models_root`（目前由 Navi 提供）。
2. 环境变量 `LUMEN_MODELS_DIR`。
3. 平台默认目录：
   - macOS：`~/Library/Application Support/Lumen/models`
   - Windows：`%USERPROFILE%\.lumen\models`
   - 其他平台：`~/.lumen/models`

用户主目录依次从 `HOME`、`USERPROFILE`、`HOMEDRIVE + HOMEPATH` 解析；均不可用时才允许退回系统临时目录。

`asr.models_root` 只允许在用户显式修改时持久化。应用不得把一次运行中由环境变量或平台默认值推导出的根目录自动写回配置，否则会遮蔽后续的 `LUMEN_MODELS_DIR` 变更。

## 2. 目录布局

规范安装目标：

```text
<LUMEN_MODELS_ROOT>/
  sensevoice/
  whisper/
  <其它用户管理的一级子目录>/
```

应用必须检查共享根目录下的一级子目录。任何满足模型就绪条件的子目录都必须作为 `lumen-shared` 候选，不要求目录名固定。

## 3. 模型解析优先级

每个引擎按以下顺序解析：

1. 持久化的 `asr.model_dir`，但仅在目录对当前引擎有效时使用。
2. 引擎环境变量：
   - `LUMEN_SENSEVOICE_DIR`
   - `LUMEN_WHISPER_DIR`
3. 规范共享目录 `<root>/sensevoice` 或 `<root>/whisper`。
4. 共享根目录下其它就绪的一级子目录。
5. 旧版 Lumen 与 coli 目录。
6. 对应的规范共享目录，作为尚未安装时的默认目标。

Navi 可继续兼容 `LUMEN_NAVI_SENSEVOICE_DIR` 和 `LUMEN_NAVI_WHISPER_DIR`，但跨应用变量仍是 `LUMEN_SENSEVOICE_DIR` 与 `LUMEN_WHISPER_DIR`。

## 4. 就绪条件

SenseVoice 目录必须同时包含：

- `model.int8.onnx`、`model.onnx` 或 `sensevoice.onnx` 之一；
- `tokens.txt`。

Whisper 目录必须同时包含：

- 文件名含 `encoder` 且扩展名为 `.onnx` 的文件；
- 文件名含 `decoder` 且扩展名为 `.onnx` 的文件；
- 文件名含 `tokens` 且扩展名为 `.txt` 的文件。

未就绪目录不得作为用户可用模型；规范共享目录即使尚不存在，也可以作为“下载目标”展示。

## 5. Legacy 兼容

所有平台都必须扫描以下旧根目录，避免跨平台迁移或升级后重新下载：

- `~/Library/Application Support/LumenAsr/models`
- `~/Library/Application Support/LumenNavi/models`
- `~/.lumen-asr/models`
- `~/.lumen-navi/models`
- `~/.coli/models` 下已知的 SenseVoice / Whisper 包目录

发现旧模型时只允许读取和选择，不得自动移动、删除或复制。用户选择后必须持久化当前引擎与 `asr.model_dir`。

## 6. 下载与跨进程互斥

SenseVoice 新下载必须安装到 `<root>/sensevoice`。Lumen 应用必须遵守同一安装协议：

1. 在共享根目录打开 `.sensevoice-install.lock`，获取操作系统级独占文件锁。
2. 未获取锁的应用等待并允许用户取消，不得启动第二次下载。
3. 获取锁后再次检查 `<root>/sensevoice`；若另一应用已完成安装，直接复用。
4. 下载与解压使用进程唯一的临时路径：
   - `.<archive-name>.<pid>.part`
   - `.sensevoice-extract-<pid>`
5. 解压结果通过就绪校验后，才发布到规范目录。
6. 发布、最终校验和临时文件清理完成前不得释放安装锁。
7. 进程退出或崩溃后，操作系统必须自动释放文件锁；锁文件本身可以保留。

进程内的原子布尔量只能用于本应用的按钮防重，不能替代跨进程文件锁。

## 7. 配置与界面

Onboarding 和 Settings 必须：

- 显示当前解析出的共享根目录；
- 按当前引擎列出所有就绪候选，包括 shared、legacy 和 coli；
- 允许粘贴任意本地目录，并在验证就绪后使用；
- 把选择持久化为当前引擎与 `asr.model_dir`；
- 不通过修改当前进程环境变量来冒充持久化。

下载始终进入共享规范目录，不因用户选择了 legacy 或自定义模型而改变安装目标。

## 8. 契约测试

两个仓库至少必须覆盖：

- 共享根目录下自定义一级子目录可被发现；
- macOS 风格与点目录风格的 legacy 根均在兼容列表；
- 缺失的规范目录仍作为安装目标返回；
- 同一共享根同时只能获得一个安装锁；
- `asr.model_dir` 配置序列化后保持不变；
- 两份 `SHARED_MODELS_CONTRACT.md` 内容逐字一致。
