# Token Monitor Lite

Windows 10/11 x64 本机用量托盘，使用 Rust、Tauri v2 和 Svelte 5。支持 Codex、Antigravity CLI／IDE SQLite、DeepSeek Harness。

## 来源与致谢

Token Monitor Lite 受 Token Monitor 启发，是按个人的 Windows 本机用量统计需求制作的精简版。感谢 Javis 和 Token Monitor 的贡献者开源原项目。Lite 延续原项目的主要界面与交互，并复用了部分图标。[Token Monitor 原项目](https://github.com/Javis603/token-monitor) · [图标许可](licenses/ICON-NOTICES.md)

## 使用

1. 解压完整 ZIP，运行 `TokenMonitorLite.exe`。详情窗口依赖 Microsoft Edge WebView2 Evergreen Runtime。
2. 托盘悬停查看简写的今日 Token 和估算成本。左键在托盘图标旁打开或关闭详情，点到窗口之外时详情自动收起。右键打开、刷新、退出。Win11 优先使用系统圆角和毛玻璃，Win10 按当前显示器的缩放比例裁切窗口四角。毛玻璃不可用时，窗口仍显示圆角半透明渐变。右下角齿轮打开设置，按 Esc 返回原来的视图。
3. 右上用 `DAY`、中间页签和 `TOTAL` 切换统计周期。在「设置 → 采集 → 统计范围」选择本周、近 7 天或近 30 天（默认），中间页签对应 `WEEK`、`7D` 或 `30D`。本周按系统的周首日计算，近 7 天和近 30 天均包含今天。会话支持 `DAY`、`MONTH` 和 `TOTAL`。
4. 左下点击当前视图依次切换主页、额度、工具、模型、会话，箭头直接选页。首页额度区任意位置可进入额度页，点工具名可定位到对应工具。工具页展示 Token 用量，工具、模型和提示摘要均可展开。会话支持按工具筛选、搜索和翻页，全页 Token 用 K／M／B 简写。

窗口较矮时，可用滚轮或触控板滚动内容。

工具和模型的展开行按输入总量计算缓存命中与未命中的比例。缓存写入计入未命中，推理计入输出。

首页「活动」和「趋势」统计截至昨天的 Token 用量。在托盘打开首页时，Lite 在创建窗口前读取 SQLite 中的活动缓存，随首页展示。缺少当天缓存时，窗口先显示，后台补算历史。在活动图上悬停某天，可查看以 K／M／B 简写的该日 Token 总量。

首页最近会话把可用的模型、思考等级、时间、调用次数、缓存命中和生成速率排在同一行。会话列表和详情顶部显示当前周期的会话平均生成速率。Codex 的上下文占用取自最近一次请求，活跃会话结束一段时间后收起。展开提示摘要可查看逐轮明细，可点击会话 ID 复制。费用显示在会话层。

逐轮计时可读取本机 CC Switch 数据库中与响应 ID 对应的路由记录。路由速率取首字到结束，输出至少 100 Token 才显示。思考未外露且有推理计数时，速率注明近似。日志估算以 `≈` 标记，速率含首字等待，输出至少 200 Token 才显示。Antigravity 使用 SQLite 记录的首字等待与后续输出耗时。Codex 需要新版日志的输出项开始时间，开头直接调用工具时省略首字估算。

费用是 API 价格估算。部分模型或 Token 项缺价时显示已计价小计，全部缺价时显示暂无估价。价格快照取自 2026-11-20（北京时间）的 [LiteLLM 模型价表](https://github.com/BerriAI/litellm/blob/3e00b708d12ebddfaff33e046d3cc647d3c1f012/model_prices_and_context_window.json)。

从现存的工具日志重算。Codex 的原生日志目录包括 `sessions` 和 `archived_sessions`。Antigravity 读取 CLI、IDE 的会话数据库，包含 `.gemini/antigravity-ide` 和 `.gemini/antigravity-backup` 下的原生 SQLite。打开 Antigravity 后，发起模型请求才会产生当日 Token。旧 `.pb` 来源暂不可读，旧 Token Monitor 归档继续在 Electron 版查看。Lite 使用自己的 `%LOCALAPPDATA%\TokenMonitorLite` 数据目录，提示摘要按需从源记录读取。

额度页按工具和模型显示剩余比例、余额和重置倒计时。同一模型的 5 小时和每周额度通常并排，空间不足时上下排列。悬停或点击某项额度，查看该项的剩余走势和近 14 天每日用量。图表卡片贴在主窗口侧边，与当前额度行对齐。每日用量来自本机观测到的变化，跨日的变化单独显示。Codex 可从近期本地日志补齐观测，其余工具从首次成功读取起积累。历史记录保存在 Lite 数据目录的 `quota-history.sqlite`。

Codex 和 Antigravity 的 5 小时、每周窗口在重置时间下显示预计可用的 Token 总量和 API 等值美元。悬停或键盘聚焦圆形 `!`，可看本周期本机已记录的 Token（K／M／B 简写）和估价。启用该工具的用量采集和额度读取后，Lite 启动时取基线，之后随用量变化补采，按额度刷新间隔合并。托盘空闲期间，待补采的额度随用量检查一起处理。首次在后台从近 15 天的可读日志补建逐次用量，之后只重读变化的会话。

预测按账号、套餐、额度组和窗口分别计算。起步时使用窗口平均，例如本机 200K 个计入额度的 Token 对应下降 10 个百分点，预测满额容量为 2M Token。本期有效扣减累计达到 5 个百分点后采用本期数据，此前可使用近 15 天的历史参考。样本不足时仍可查看已用量。

模型的基础消耗从本机额度下降中学习。每个参与预测的模型至少需要 2 个只有该模型的扣减区间，合计达到 5 个百分点。Lite 先用更早的数据预估后续扣减，至少比较 3 次且平均误差不高于窗口平均，才按最近最多 20 次计入额度的调用（至少 3 次）估算**剩余额度**能用多少 Token，再加上已消耗部分。悬停预测旁的 `!` 可看到当前使用的方法。跨设备用量、缓存命中率或任务类型变化仍会影响估算。

Codex 日志明确记录速度档位时，Lite 使用 OpenAI 公布的同模型订阅倍率，例如 GPT-6.2 Astra 和 Sol 的 Fast 为 2 倍、Ultrafast 为 6 倍。未记录档位的请求按自身观测校准。免费自动审批检查计入本机已用 Token，预测容量只统计消耗额度的调用。[OpenAI 订阅额度与速度倍率](https://learn.chatgpt.com/docs/pricing#speed-multipliers) · [Codex 自动审批说明](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan)

Antigravity 按服务返回的模型组分别校准 5 小时与每周额度。Google 说明同组模型共享两种窗口，基础额度取决于工作量和可用容量。预测金额统一按内置 API 基础价表折算。缺价时只显示 Token 和已计价小计。[Google 模型额度分组](https://antigravity.google/docs/cli/commands/usage) · [Google 基础额度说明](https://antigravity.google/docs/plans)

点击预测旁的「重置」，可让对应工具、模型组的单个窗口重新采样。本周期已用 Token、估价和额度历史均保留。从重置后的首次有效额度读数开始，累计有效扣减达到 5 个百分点即可重新显示预测。重启 Lite 后仍按这个起点计算。

运行中的 Antigravity 提供实时额度和重置时间。Lite 优先读取分组额度，旧版服务可回退到模型额度。套餐名暂时读不到时仍显示有效额度，包括剩余 0% 的窗口。每轮 Codex 在线刷新读取一次登录信息，并行查询 5 小时／每周额度和 Banked Reset，合并后更新面板。常规额度请求失败或响应没有可用窗口时，使用本机日志或上次可用读数。服务只返回一个窗口时，按其时长显示对应额度。用量变化时也可从本机日志更新常规额度。点 Banked Reset 旁的 `i` 查看各次的到期明细。配置 `DEEPSEEK_API_KEY` 或 `DEEPSEEK_KEY` 后可显示 DeepSeek API 余额。

启动、面板定时刷新和托盘空闲检查都会并行查询已启用的额度。从托盘打开主页或额度页时先显示上次结果，只有达到「设置 → AI 工具额度 → 刷新频率」才发起新一轮查询。托盘空闲检查即使没有新日志或已关闭所有用量采集，也会查询额度。在任一视图点右下角「刷新」，或从托盘菜单选「刷新」，也会立即发起同样的查询。Antigravity 向运行中的本机服务请求最新额度，DeepSeek 使用已配置的密钥查询余额。某项查询失败时，保留该项最近的可用读数。

在「设置 → 网络代理」可配置 Lite 自己的代理。支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H，例如 `http://127.0.0.1:7890`。只填主机和端口时按 HTTP 处理。保存后，新发起的 Codex 在线额度、Banked Reset、DeepSeek 余额及 Lite 更新检查和下载使用此代理。需要账号密码的代理可通过 `HTTPS_PROXY` 环境变量配置。

自定义代理使用界面内的绕过列表。逗号分隔域名、IP、CIDR 网段或 `*`。主机可带端口，域名规则包含其子域。网段按目标 IP 字面量匹配，例如 `10.0.0.0/8` 或 `2001:db8::/32`。回环地址和链路本地地址直连，Antigravity 本机 RPC 走直连。代理地址留空并保存后，回环地址和 `NO_PROXY` 匹配的目标直连，其余请求依次使用 `--proxy-server` 启动参数、按协议选择的 `HTTP_PROXY` 或 `HTTPS_PROXY`、`ALL_PROXY` 和 Windows 代理。构建时的 Tokscale 下载读取构建进程的代理环境。

Windows 快捷方式的「目标」可参照下例填写，将路径换成实际安装位置。更改参数后，从托盘退出 Lite，再用该快捷方式启动。

```text
"D:\Program Files\TokenMonitor-Lite\TokenMonitorLite.exe" --proxy-server=http://127.0.0.1:1080
```

启动参数和环境变量均未配置代理时，先读取当前 Windows 用户的代理设置（含 PAC），再回落到 WinHTTP 默认配置。HTTP 代理由 WinHTTP 使用，HTTPS、SOCKS5 或带凭据的代理使用系统 `curl.exe`。`socks5h://` 由代理解析域名。`socks5://` 使用本机 DNS，连接失败且未收到 HTTP 响应时，再用同一代理解析。

Lite 日志位于 `%LOCALAPPDATA%\TokenMonitorLite\logs\token-monitor-lite.log`。设置了 `TOKEN_MONITOR_LITE_DATA_DIR` 时，日志在该目录的 `logs` 下。日志使用 UTC，启动时接着写。文件超过 10 MiB（10,485,760 字节）后，下次写入前将原文件归档到同目录，例如 `token-monitor-lite-20261121-113104-252763900Z.log`，再写入新的 `token-monitor-lite.log`。

日志记录应用启动、正常退出、用量扫描失败和额度诊断。排查额度时可重新启动 Lite，再打开主页或额度页，点右下角刷新。

- `quota startup` 记录启动采集是否排入计划及工具开关。`codex_quota` 记录 Codex 共用的登录检查，`codex_usage` 和 `banked_reset` 分别记录两项请求的代理来源、HTTP 状态和失败阶段。`trigger=startup`、`trigger=panel`、`trigger=manual`、`trigger=tray_idle` 区分启动、面板自动刷新、手动和托盘空闲查询。
- `quota read` 记录读取来源和结果，包括额度记录时间 `observed_at`、读到时的年龄 `age_ms`、窗口剩余百分比和重置时间。Codex 会记录扫描文件数量和读取错误，Antigravity 会区分未发现端点、RPC 失败和响应缺少有效额度。
- `refresh` 记录面板检查是否命中缓存和等待时间。`quota publish` 记录后端采用的读数时间及是否变新，`panel` 记录面板采用的 Codex 和 Antigravity 读数时间。

Banked Reset 原有的 HTTP 状态、可用次数和缓存来源继续记录。日志仅记录诊断所需字段，省略令牌、账号 ID、代理地址及凭据和原始响应。

Codex 常规额度、Banked Reset 和 DeepSeek 的外网请求各自等待最多约 20 秒，并为 PowerShell 启动单独预留时间。三项可以并行读取，一项超时仍保留其他已返回的额度。

## 设置

- **外观**选择跟随系统、简体中文或 English。语言设置保存在本机，切换后立即应用到详情窗口和托盘。也可调节背景不透明度（65%–100%）和标准、较大、最大三档字号。
- **采集**选择追踪的工具和托盘空闲最长间隔。打开详情窗口或将鼠标移入托盘图标时立即刷新今日用量，窗口打开或鼠标停留期间，日志变化后约 3–5 秒开始采集。窗口收起且鼠标离开后，默认依次等待 1、3、5、10、20 分钟检查，之后每 20 分钟检查一次。期间的日志写入合并到下次检查，间隔继续递增。日志监看正常且没有变化时跳过用量扫描和用量库写入，仍查询已启用的额度，成功读数写入额度历史。再次打开、悬停或使用托盘菜单刷新会重置间隔。原来的实时设置对应默认 20 分钟上限，已选的 5、15、30 分钟对应各自的托盘上限。点工具名可查看日志目录和今日用量。切换追踪工具会从现存日志重新计算今日和当前周期的统计。
- **AI 工具额度**独立选择读取哪些工具的额度和主页、额度页的刷新频率。点工具名查看接入方式。收起窗口且鼠标离开托盘后，额度查询跟随「托盘空闲最长间隔」。设置保存在本机。
- **网络代理**显示代理来源，可保存代理地址和绕过列表。设置保存在 Lite 数据目录的 `proxy.json`。
- **关于 Lite**显示当前版本、可用新版、最近检查时间和发布页。自动更新默认开启，便携版在启动后及运行期间每 24 小时检查一次。通过校验的新版会在后台准备好，点击「重启并更新」后才安装。关闭自动更新后可手动检查和下载。

更新需要应用目录可写。Lite 在原目录替换应用、Tokscale 和随包文件，然后重新启动，沿用原有设置和用量数据。首次使用带更新功能的版本时，请从发布页下载并解压完整 ZIP。

## 构建

需要 Windows x64、Node.js 22.15+（含 npm）、Rust MSVC 工具链、Windows SDK 与 `tar.exe`。

### 一键打包

双击仓库根目录的 `package-win.cmd`。脚本会检查工具链、准备前端依赖、校验 Tokscale 并构建便携版。生成的 ZIP 位于 `release/`，运行结果会保留在命令窗口里。依赖已安装时，也可以从终端执行 `npm run dist:win`。

### GitHub 发布

同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 和 `src-tauri/tauri.conf.json` 中的应用版本并提交。推送与版本匹配的 `v` 标签后，Actions 会在 Windows x64 上执行检查和打包，将 `Token-Monitor-Lite-<版本>-win-x64.zip` 和含版本号、ZIP SHA-256 的 `update.json` 上传到对应的 GitHub Release。

```powershell
git tag -a v0.1.19 -m "Token Monitor Lite 0.1.19"
git push origin v0.1.19
```

重新运行可补传缺少的资产。更新清单以该 Release 上的 ZIP 为准。带预发行后缀的版本发布为 Pre-release，Lite 的更新只跟随稳定版。发布后可通过项目 Releases 的 `latest/download/update.json` 匿名读取清单，并核对其中的 SHA-256 与公开 ZIP 一致。

### 开发与验证

新增 HTTP 调用使用 `scripts/windows-http.ps1` 的 `Invoke-LiteHttpRequest`。它接收目标 URI、方法、请求头和请求体，下载文件时传入 `OutFile` 和 `MaxBytes`。代理选择集中在 `Get-LiteProxy`，各提供方只负责构造请求和解析响应。

在仓库根目录运行。

```powershell
npm ci
npm run check
npm run dev
```

```powershell
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --features custom-protocol
```

测试时可为新进程设置 `TOKEN_MONITOR_LITE_DATA_DIR`（应用数据）与 `TOKEN_MONITOR_LITE_SOURCE_HOME`（工具日志），两者均使用绝对路径。追加 `--open` 可直接打开详情。

用 `node tests/make-fixture.cjs tmp/新的样例目录` 生成三个工具的脱敏样例和期待总数，含跨周期记录和 100 条以上的会话。将输出中的 `home` 设为测试日志目录。

### Tokscale 版本

`scripts/tokscale.json` 锁定发布仓库、标签、源码提交、Windows x64 资产与 SHA-256。当前使用基于 4.23.0 的 Token Monitor 分支构建。`npm run ensure:tokscale` 校验本地 `vendor/tokscale.exe`，需要时下载固定发布的资产。离线准备时，可以提前将对应资产保存到该路径。[固定版本发布页](https://github.com/Javis603/tokscale/releases/tag/token-monitor-648cc594)

升级时，将 `version`、`releaseRepo`、`releaseTag`、`commit`、`asset` 和 `sha256` 一起更新为目标构建的数据。用样例核对时区分桶、会话与模型分组，以及 Codex、Antigravity 和 DeepSeek Harness 的统计，再重新打包。Lite 的扫描器调用需要支持 `--bucket-timezone` 和 `--group-by client,workspace,session,model`。
