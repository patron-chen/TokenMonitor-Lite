# Token Monitor Lite

Windows 10/11 x64 本机用量托盘，使用 Rust、Tauri v2 和 Svelte 5。支持 Codex、Antigravity CLI／IDE SQLite、DeepSeek Harness。

## 来源与致谢

Token Monitor Lite 受 Token Monitor 启发，是按个人的 Windows 本机用量统计需求制作的精简版。感谢 Javis 和 Token Monitor 的贡献者开源原项目。Lite 延续原项目的主要界面与交互，并复用了部分图标。[Token Monitor 原项目](https://github.com/Javis603/token-monitor) · [图标许可](licenses/ICON-NOTICES.md)

## 使用

1. 解压完整 ZIP，运行 `TokenMonitorLite.exe`。详情窗口依赖 Microsoft Edge WebView2 Evergreen Runtime。
2. 托盘悬停查看简写的今日 Token 和估算成本。左键在托盘图标旁打开或关闭详情，点到窗口之外时详情自动收起。右键打开、刷新、退出。窗口使用圆角裁切的 Windows 原生毛玻璃，系统材质不可用时显示半透明渐变。右下角齿轮打开设置，按 Esc 返回原来的视图。
3. 右上切换 `DAY / MONTH / TOTAL`。再次点击选中的中间页签可选本月、本周、近 7 天或近 30 天。会话支持今日、本月、全部。
4. 左下点击当前视图依次切换主页、额度、工具、模型、会话，箭头直接选页。首页额度区任意位置可进入额度页，点工具名可定位到对应工具。工具页展示 Token 用量，工具、模型和提示摘要均可展开。会话支持按工具筛选、搜索和翻页，全页 Token 用 K／M／B 简写。

窗口较矮时，可用滚轮或触控板滚动内容。

工具和模型的展开行按输入总量计算缓存命中与未命中的比例。缓存写入计入未命中，推理计入输出。

在首页「活动」中悬停某天，可查看以 K／M／B 简写的当日 Token 总量。

首页最近会话把可用的模型、思考等级、时间、调用次数、缓存命中和生成速率排在同一行。Codex 的上下文占用取自最近一次请求，活跃会话结束一段时间后收起。展开提示摘要可查看逐轮明细，可点击会话 ID 复制。费用显示在会话层。

逐轮计时可读取本机 CC Switch 数据库中与响应 ID 对应的路由记录。路由速率取首字到结束，输出至少 100 Token 才显示。思考未外露且有推理计数时，速率注明近似。日志估算以 `≈` 标记，速率含首字等待，输出至少 200 Token 才显示。日志含有相应时间点时才显示估算首字。Codex 需要新版日志的输出项开始时间，开头直接调用工具时省略首字估算。

费用是 API 价格估算。部分模型或 Token 项缺价时显示已计价小计，全部缺价时显示暂无估价。价格快照取自 2026-11-20（北京时间）的 [LiteLLM 模型价表](https://github.com/BerriAI/litellm/blob/3e00b708d12ebddfaff33e046d3cc647d3c1f012/model_prices_and_context_window.json)。

从现存的工具日志重算。Codex 的原生日志目录包括 `sessions` 和 `archived_sessions`。Antigravity 读取 CLI、IDE 的会话数据库，包含 `.gemini/antigravity-ide` 和 `.gemini/antigravity-backup` 下的原生 SQLite。打开 Antigravity 后，发起模型请求才会产生当日 Token。旧 `.pb` 来源暂不可读，旧 Token Monitor 归档继续在 Electron 版查看。Lite 使用自己的 `%LOCALAPPDATA%\TokenMonitorLite` 数据目录，提示摘要按需从源记录读取。

额度页按工具和模型显示剩余比例、余额和重置倒计时，同一模型的 5 小时和每周额度并排。悬停或点击某项额度，查看该项的剩余走势和近 14 天每日用量。每日用量来自本机观测到的变化，跨日的变化单独显示。Codex 可从近期本地日志补齐观测，其余工具从首次成功读取起积累。历史记录保存在 Lite 数据目录的 `quota-history.sqlite`。

运行中的 Antigravity 提供实时额度和重置时间。Codex 显示最近的本地额度记录，并从已登录账号读取 Banked Reset 的可用次数和到期时间。点 `i` 查看各次的到期明细。配置 `DEEPSEEK_API_KEY` 或 `DEEPSEEK_KEY` 后可显示 DeepSeek API 余额。窗口重开先显示托盘进程中的上次结果，再刷新已过期的额度。个别工具读取失败时保留其上次结果。

## 设置

- **外观**调节背景不透明度（65%–100%）和标准、较大、最大三档字号。
- **采集**选择追踪的工具和今日采集频率。实时监看默认在日志变化后采集，定时模式使用 5、15 或 30 分钟间隔。点工具名展开日志目录和今日用量。切换追踪工具会从现存日志重新计算今日和当前周期的统计。
- **AI 工具额度**独立选择读取哪些工具的额度和刷新频率。点工具名查看接入方式。打开主页或额度页时按所选间隔刷新额度。设置保存在本机，重启后生效。

## 构建

在仓库根目录运行。需要 Windows x64、Node.js 22.15+、Rust MSVC 工具链与 Windows SDK。

```powershell
npm ci
npm run check
npm run dev
```

开发和打包使用 `scripts/tokscale.json` 固定的 Tokscale 4.23.0 构建。首次准备扫描器时下载并校验 SHA-256，之后复用 `vendor/tokscale.exe`。可用 `npm run ensure:tokscale` 单独准备扫描器。

离线构建可提前将对应的 Windows x64 资产保存为 `vendor/tokscale.exe`，打包时会校验哈希。[固定版本发布页](https://github.com/Javis603/tokscale/releases/tag/token-monitor-648cc594)

便携 ZIP 输出到 `release/`。

```powershell
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --features custom-protocol
npm run dist:win
```

测试时可为新进程设置 `TOKEN_MONITOR_LITE_DATA_DIR`（应用数据）与 `TOKEN_MONITOR_LITE_SOURCE_HOME`（工具日志），两者均使用绝对路径。追加 `--open` 可直接打开详情。

用 `node tests/make-fixture.cjs tmp/新的样例目录` 生成三个工具的脱敏样例和期待总数，含跨周期记录和 100 条以上的会话。将输出中的 `home` 设为测试日志目录。
