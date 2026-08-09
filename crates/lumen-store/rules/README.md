# Category rules — 文本 / 映射规则引擎使用说明

Lumen Navi 的分类分两层：

| 层 | 在哪 | 怎么更新 |
|----|------|----------|
| **引擎（固定）** | `crates/lumen-store/src/rule_engine.rs` | 改代码、重编译（少见） |
| **规则（数据）** | 本目录 JSON；运行时在 `$data_dir/rules/` | **只改文件即可** |

引擎负责：怎么匹配、谁优先、如何 reload。  
规则负责：哪些关键词 / genre / bundle 对应哪个类目。

---

## 1. 这些规则作用在什么文本上？（很重要）

文本规则 **只吃「应用元数据」**，不会扫邮件正文、OCR 全文或聊天内容：

| 输入 | 是否走 `text_rules` |
|------|---------------------|
| Homebrew Cask `desc` / `name` / `homepage` | ✅ enrichment |
| iTunes / App Store `primaryGenreName` | ✅ 走 `itunes_genre_rules`（不是 text） |
| Info.plist `LSApplicationCategoryType` | ✅ 走 `ls_uti_rules` |
| 前台窗口标题、邮件 body、截图 OCR | ❌ **不走**这套 text 规则 |
| 用户在 UI 里写的 category rules | ❌ 另一套（`activity.category_rules`） |

所以「邮件里出现 support」**不会**因为本文件被标成 Utilities。  
误伤面主要在：**Homebrew 一句话 desc 写得很闲、很营销** 时。

实时分类优先级（引擎固定）：

```text
用户规则
  → app_catalog（bundle / 名 / 域名）
  → LSApplicationCategoryType（本机 plist）
  → enrichment 缓存（brew / iTunes 解析结果）
  → 产品族启发式
  → Uncategorized
```

---

## 2. 文件一览

| 文件 | 作用 |
|------|------|
| `category_mapping.v1.json` | 文本 genre、`itunes_genre_rules`、`ls_uti_rules` |
| `app_catalog.v1.json` | 已知 `bundle_id` / 应用名 / 域名目录 |
| `README.md` | 本文 |

仓库内的 JSON 会 `include_str!` 打进二进制作默认；**首次打开 store** 时复制到可写目录。

### 运行时路径

```text
$data_dir/rules/category_mapping.v1.json
$data_dir/rules/app_catalog.v1.json
```

macOS 常见：

```text
~/Library/Application Support/LumenNavi/rules/
```

改这里即可。每轮 category enrichment 会 `reload`；也可代码调用：

```rust
store.reload_category_rules()?;
```

JSON 非法 → 打 warn，**回退 embedded 默认**，不会把进程打挂。

---

## 3. 文本规则引擎（`text_rules`）

### 3.1 优先级

**数组顺序 = 优先级**。从上到下，**第一条命中即停**。

因此：更具体的短语放上面，更宽的词放下面（或加 `none` 排除）。

### 3.2 字段语义

| 字段 | 语义 | 例子 |
|------|------|------|
| `id` | 可读名，仅日志/调试 | `"remote-access-tools"` |
| `any` | **任一**子串命中即可（OR） | `["remote desktop", "vnc client"]` |
| `all` | **全部**子串都要出现（AND） | `["ai", "code"]` |
| `none` | 出现任一则**整条否决** | `["remote work", "customer support"]` |
| `all_groups` | 每组内 OR，组与组 AND | 见下 |
| `category` | 产品类目名 | `"Utilities"` |
| `level` | `productive` \| `neutral` \| `distracting` | 可省略 |

匹配前会把 haystack **小写**；规则里的关键字也会规范成小写。  
匹配方式是 **子串 contains**（不是整词 tokenize），所以要警惕短词（`ai`、`ide ` 两侧带空格等）。

### 3.3 `all_groups` 示例

```json
{
  "id": "example-and-or",
  "all_groups": [
    ["remote"],
    ["desktop", "control"]
  ],
  "category": "Utilities",
  "level": "neutral"
}
```

含义：包含 `remote`，**并且**（包含 `desktop` **或** `control`）。

> **准确率提示**：`remote` + `support` / `connect` 这类组合**过于宽松**  
> （如 *“notes app for remote work support”*），默认规则里**已不用**宽 compound。  
> 远程类只保留 **多词短语**（`remote desktop`、`remote access`…）+ `none` 排除。

### 3.4 完整一条

```json
{
  "id": "remote-access-tools",
  "any": [
    "remote desktop",
    "remote access",
    "remote control",
    "vnc client",
    "rdp client"
  ],
  "none": [
    "remote work",
    "remote team",
    "customer support",
    "tech support"
  ],
  "category": "Utilities",
  "level": "neutral"
}
```

### 3.5 怎么加规则（推荐流程）

1. 先拿真实元数据：  
   `curl -s https://formulae.brew.sh/api/cask/<token>.json | jq .desc,.name`
2. 判断是「缺规则」还是「源数据就没有」  
   - 有 desc/genre → 改 `category_mapping.v1.json`  
   - 只有 bundle、文案瞎写 → 改 `app_catalog.v1.json`  
3. **优先多词短语**，少用单词（`support`、`control`、`connect`、`remote` 单独出现）
4. 用 `none` 挡掉已知误伤面
5. 把新规则插在「更具体」的位置（靠前）
6. 保存到 **`$data_dir/rules/`**（或改仓库默认后再发版）
7. 等 enrichment 重跑，或重启 app / 调 `reload_category_rules`

**不要**为单个 app 在 Rust 里写 `if bundle_id == ...`，除非所有外部元数据都拿不到。

### 3.6 自测（不改代码）

在仓库根：

```bash
# 引擎 + 规则单元测试（含 teamwork / remote desktop / 文件覆盖）
cargo test -p lumen-store --lib rule_engine categorization::tests::text_hint
```

临时验证一条逻辑：编辑 `rules/category_mapping.v1.json` 后跑上述测试；  
或改 `$data_dir/rules/` 后触发 enrichment。

---

## 4. 准确率：哪些写法容易误伤？

### 4.1 高风险（避免）

| 写法 | 问题 |
|------|------|
| `any: ["support"]` | 几乎所有软件 desc 都会写 support |
| `any: ["control"]` | version control、parental control… |
| `any: ["remote"]` 单独 | remote work / remote team |
| `all_groups: [["remote"],["support"\|"connect"]]` | *remote work support*、*connect remote employees* |
| 过短子串 `ai`、`ide` 不带边界 | 误进 random 单词 |

### 4.2 默认文件里的取舍（当前策略）

| 类别 | 策略 |
|------|------|
| **远程桌面** | 只用 **多词短语**（remote desktop / remote access / vnc client…）；`none` 排除 remote work、customer support 等；**已删除** `remote`∧`support` 宽 compound |
| **沟通协作** | teamwork / collaboration / video conferencing 等；`messaging` 排除 `message queue`；`chat` 排除 chatbot/chatgpt |
| **浏览器** | browser 排除 file browser |
| **开发** | 多词为主；`ai`+`code` 用 `all` 双条件 |

### 4.3 预期召回 vs 精度

这是 **冷启动启发式**，不是 Timing 级项目规则：

- **宁可漏（Uncategorized / 其它源）**，也不要用单词把无关 app 扫进 Utilities  
- 漏了：靠 catalog 一行、或用户规则、或以后加更稳的短语  
- 错了：在对应 `text_rules` 上加 `none`，或把更具体的规则挪到前面  

UU 远程一类仍能靠 brew desc 里的 **`remote desktop`** 短语命中，不依赖宽 `support`。

### 4.4 若仍不够准

升级路径（仍尽量不改引擎）：

1. 加长短语、加 `none`  
2. `app_catalog.v1.json` 钉 bundle（最后手段）  
3. 以后再考虑：词边界、简单正则（需扩展引擎 schema）  
4. 不要把 text 规则套到 window title 上扫聊天内容  

---

## 5. iTunes genre 与 LS UTI

### `itunes_genre_rules`

```json
{ "genre": "utilities", "category": "Utilities", "level": "neutral" }
```

- `genre` 与 App Store `primaryGenreName` **忽略大小写全等**（不是子串）  
- 未列出的 genre：引擎侧可能有极低置信度兜底，但应优先在本表补全  

### `ls_uti_rules`

```json
{ "uti": "developer-tools", "category": "Development", "level": "productive" }
```

- 可写裸后缀 `developer-tools`，或完整 `public.app-category.developer-tools`  
- 本机 Info.plist 有值时优先于 enrichment 文本  

---

## 6. App catalog（`app_catalog.v1.json`）

已知身份的精确表，**不是**文本猜：

```json
{
  "field": "bundle_id",
  "value": "com.mitchellh.ghostty",
  "category": "Development",
  "level": "productive"
}
```

| `field` | 匹配方式 |
|---------|----------|
| `bundle_id` | 精确（大小写不敏感） |
| `app_name` | 精确（大小写不敏感） |
| `domain` | 精确（从 URL 抽出的域名） |
| `url` / `title` | **子串** contains |

顺序同样是先匹配先生效。用户 UI 规则永远压过 catalog。

---

## 7. 和「用户规则」的关系

| 类型 | 存储 | 谁改 |
|------|------|------|
| 产品默认 catalog / mapping | `$data_dir/rules/*.json` | 开发者 / 高级用户 / 发版 |
| 用户覆盖 | SQLite `kv`：`activity.category_rules` | App UI |

用户改「这个 app 算写作」→ UI 写用户规则，**不要**塞进 mapping JSON。

---

## 8. 发版与同步

1. 在仓库改 `crates/lumen-store/rules/*.json` → 下次编译进 embedded  
2. 用户机器上若已有 `$data_dir/rules/` 旧文件，**不会自动覆盖**（避免冲掉本地定制）  
3. 需要推送新默认时：文档说明手动合并，或以后做 version bump 迁移（尚未做）

---

## 9. 快速对照：我该改哪？

| 现象 | 动作 |
|------|------|
| brew desc 写着 teamwork 但仍 Uncategorized | 查 enrichment 是否 failed；确认 mapping 含 teamwork；reload |
| 远程类 desc 含 *remote desktop* 却分错 | 查 `text_rules` 顺序是否被更前规则截胡 |
| 误把「remote work 协作工具」打成 Utilities | 在 remote 规则 `none` 加 `remote work`（已默认有） |
| 某国产 app 无 brew/iTunes/LS | `app_catalog` 加 bundle；仍不要写死进 Rust |
| 想支持正则 / 整词边界 | 改引擎 schema（编译），再扩展 JSON |

---

## 10. 引擎实现入口（给开发者）

| 符号 | 文件 |
|------|------|
| 匹配与 reload | `src/rule_engine.rs` |
| classify 流水线 | `src/categorization.rs` |
| brew / iTunes enrichment | `src/enrichment.rs` |
| open 时 seed 规则 | `SqliteStore::open` → `install_and_load_rules` |
