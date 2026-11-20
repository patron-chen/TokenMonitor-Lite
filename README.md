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

在首页「活动」中悬停某天，可查看以 K／M／B 简写的当日 Token 总量。

首页最近会话把可用的模型、思考等级、时间、调用次数、缓存命中和生成速率排在同一行。会话列表和详情顶部显示当前周期的会话平均生成速率。Codex 的上下文占用取自最近一次请求，活跃会话结束一段时间后收起。展开提示摘要可查看逐轮明细，可点击会话 ID 复制。费用显示在会话层。

逐轮计时可读取本机 CC Switch 数据库中与响应 ID 对应的路由记录。路由速率取首字到结束，输出至少 100 Token 才显示。思考未外露且有推理计数时，速率注明近似。日志估算以 `≈` 标记，速率含首字等待，输出至少 200 Token 才显示。Antigravity 使用 SQLite 记录的首字等待与后续输出耗时。Codex 需要新版日志的输出项开始时间，开头直接调用工具时省略首字估算。

费用是 API 价格估算。部分模型或 Token 项缺价时显示已计价小计，全部缺价时显示暂无估价。价格快照取自 2026-11-20（北京时间）的 [LiteLLM 模型价表](https://github.com/BerriAI/litellm/blob/3e00b708d12ebddfaff33e046d3cc647d3c1f012/model_prices_and_context_window.json)。

从现存的工具日志重算。Codex 的原生日志目录包括 `sessions` 和 `archived_sessions`。Antigravity 读取 CLI、IDE 的会话数据库，包含 `.gemini/antigravity-ide` 和 `.gemini/antigravity-backup` 下的原生 SQLite。打开 Antigravity 后，发起模型请求才会产生当日 Token。旧 `.pb` 来源暂不可读，旧 Token Monitor 归档继续在 Electron 版查看。Lite 使用自己的 `%LOCALAPPDATA%\TokenMonitorLite` 数据目录，提示摘要按需从源记录读取。

额度页按工具和模型显示剩余比例、余额和重置倒计时，同一模型的 5 小时和每周额度并排。悬停或点击某项额度，查看该项的剩余走势和近 14 天每日用量。每日用量来自本机观测到的变化，跨日的变化单独显示。Codex 可从近期本地日志补齐观测，其余工具从首次成功读取起积累。历史记录保存在 Lite 数据目录的 `quota-history.sqlite`。

运行中的 Antigravity 提供实时额度和重置时间。Lite 优先读取分组额度，旧版服务可回退到模型额度。套餐名暂时读不到时仍显示有效额度，包括剩余 0% 的窗口。Codex 显示最近的本地额度记录，并从已登录账号读取 Banked Reset 的可用次数和到期时间。点 `i` 查看各次的到期明细。配置 `DEEPSEEK_API_KEY` 或 `DEEPSEEK_KEY` 后可显示 DeepSeek API 余额。窗口重开先显示托盘进程中的上次结果，再刷新已过期的额度。个别工具读取失败时保留其上次结果。

Codex Banked Reset、DeepSeek 余额、Antigravity 本机 RPC 和构建时的 Tokscale 下载共用 HTTP 请求接口。回环地址和 `NO_PROXY` 匹配的域名或 IP 直连（逗号分隔，`*` 匹配全部）。其余请求优先使用 Lite 的 `--proxy-server` 启动参数，然后按目标协议读取 `HTTP_PROXY` 或 `HTTPS_PROXY`，其次使用 `ALL_PROXY`。构建时下载使用构建进程的代理环境。

Windows 快捷方式的「目标」可参照下例填写，将路径换成实际安装位置。更改参数后，从托盘退出 Lite，再用该快捷方式启动。

```text
"D:\Program Files\TokenMonitor-Lite\TokenMonitorLite.exe" --proxy-server=http://127.0.0.1:1080
```

启动参数和环境变量均未配置代理时，先读取当前 Windows 用户的代理设置（含 PAC），再回落到 WinHTTP 默认配置。HTTP 代理由 WinHTTP 使用，HTTPS、SOCKS5 或带凭据的代理使用系统 `curl.exe`。`socks5h://` 由代理解析域名。`socks5://` 使用本机 DNS，连接失败且未收到 HTTP 响应时，再用同一代理解析。

排查 Codex Banked Reset 时，打开主页或额度页，点右下角刷新。等待额度读取结束后，可分享 `%LOCALAPPDATA%\TokenMonitorLite\logs\codex-banked-reset.log`。设置了 `TOKEN_MONITOR_LITE_DATA_DIR` 时，日志在该目录的 `logs` 下。日志自动记录账号凭据读取结果、代理来源与传输方式、HTTP 状态或错误码、可用次数、缓存来源和面板是否生成对应行。日志使用 UTC 时间，达到 128 KiB 后会在下次额度读取或启动时重新开始。令牌、账号 ID、代理地址及凭据、原始响应均不写入日志。

Codex 和 DeepSeek 的外网额度请求分别等待最多约 20 秒，并为 PowerShell 启动单独预留时间。两个工具同时启用时会并行读取，一方超时仍保留另一方已返回的额度。

## 设置

- **外观**选择跟随系统、简体中文或 English。语言设置保存在本机，切换后立即应用到详情窗口和托盘。也可调节背景不透明度（65%–100%）和标准、较大、最大三档字号。
- **采集**选择追踪的工具和今日采集频率。实时监看默认在日志变化后采集，定时模式使用 5、15 或 30 分钟间隔。点工具名展开日志目录和今日用量。切换追踪工具会从现存日志重新计算今日和当前周期的统计。
- **AI 工具额度**独立选择读取哪些工具的额度和刷新频率。点工具名查看接入方式。打开主页或额度页时按所选间隔刷新额度。设置保存在本机，重启后生效。

## 构建

需要 Windows x64、Node.js 22.15+（含 npm）、Rust MSVC 工具链、Windows SDK 与 `tar.exe`。

### 一键打包

双击仓库根目录的 `package-win.cmd`。脚本会检查工具链、准备前端依赖、校验 Tokscale 并构建便携版。生成的 ZIP 位于 `release/`，运行结果会保留在命令窗口里。依赖已安装时，也可以从终端执行 `npm run dist:win`。

### GitHub 发布

同步 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 和 `src-tauri/tauri.conf.json` 中的应用版本并提交。推送与版本匹配的 `v` 标签后，Actions 会在 Windows x64 上执行检查和打包，将 `Token-Monitor-Lite-<版本>-win-x64.zip` 上传到对应的 GitHub Release。

```powershell
git tag -a v0.1.16 -m "Token Monitor Lite 0.1.16"
git push origin v0.1.16
```

重新运行可补传缺少的 ZIP，并复用已有同名资产。带预发行后缀的版本发布为 Pre-release。

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
