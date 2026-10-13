# Obscura-Hunt

[English](README.md) | **简体中文**

**Obscura-Hunt** 是独立的 Rust 无头浏览器，面向网页抓取、浏览器自动化和 AI agent。
它通过 V8 执行真实 JavaScript，原生渲染页面，并提供 CDP 和 MCP 接口，
支持 Puppeteer、Playwright 等客户端。

当自动化需要在弹窗、内嵌页面和跳转之间连续操作，而不只是读取单个网页时，
Obscura-Hunt 值得选择。本项目由 [MythHack](https://github.com/MythHack/obscura-hunt) 维护，
基于 [官方 Obscura](https://github.com/h4ckf0r0day/obscura)，保留原生引擎和 stealth 选项，
在同步上游更新的同时，持续改进浏览器操作流程。

最近一次上游同步包含
[`8edfe48`](https://github.com/h4ckf0r0day/obscura/commit/8edfe480133b383f70e17f9d4ea55e3592cdfd24)。
新增 CDP 对象句柄兼容、页面关闭与停止加载、ES 模块网络事件及响应正文，
同时保留本分支的原生 popup 和子 frame 所有权语义。

## 为什么选择 Obscura-Hunt？

如果你已经在使用官方 Obscura，以下改进能帮助自动化完成多步骤流程，
无需更换现有 CDP 或 MCP 客户端：

- **让弹窗流程保持连贯。** 打开新窗口、与原页面传递消息，并判断窗口是否关闭。
  跳转或关闭一个页面，不会清掉其他页面已经加载的 JavaScript 模块。
- **在真实内嵌文档中执行自动化。** 顶层和子 frame 的 isolated world 各自保留全局变量与原型，
  同时共享原生 DOM；点击、焦点、表单状态和事件取消可以跨 world 正确协作。
  跨源访问限制和网页自身的嵌入策略仍然生效。
- **页面跳转后继续等到结果。** 页面加载新 URL 后，定时器和异步脚本继续运行。
  MCP 等待期间会让页面正常执行，等待的任务才有机会完成。
- **点击与真实画面保持一致。** CSS `zoom`、变换后的 iframe 坐标和 3D `perspective`
  让布局、透视绘制、元素边界与命中测试使用一致的原生几何结果。
- **请求拦截不因弹窗而变样。** 弹窗沿用指定 URL 的过滤规则，
  不会意外暂停所有请求。
- **获得真实的字体加载结果。** SFNT、WOFF、WOFF2 经过实际解码与校验，损坏的字体会失败，
  不再伪装成加载成功；字体子集根据文档真正使用的文本选择。
- **读取有实际含义的 Web API 状态。** `DOMMatrix` 运算、可见性判断、可写媒体属性和
  实时 text track 列表反映各自的真实状态。

继续使用 Puppeteer、Playwright 或 MCP agent，并保留上游的 Rust/V8 原生引擎、
截图、PDF 输出和可选 stealth 传输能力。本项目在此基础上完善上述操作流程、
升级关键依赖，并持续同步官方 Obscura 更新。

## 依赖升级

与官方 Obscura 提交
[`2bda71a`](https://github.com/h4ckf0r0day/obscura/commit/2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8)
相比，本分支包含以下依赖升级。下表版本来自已提交的锁文件，列出浏览器直接使用的依赖，
并非所有间接依赖的更新。

| 用途 | 版本变化 |
| --- | --- |
| HTML 解析 | `html5ever` 0.39.0 → 0.40.1；`markup5ever` 0.39.0 → 0.40.0 |
| CSS 与选择器 | `cssparser` 0.34.0 → 0.38.0；`selectors` 0.26.0 → 0.41.0；`servo_arc` 0.4.3 → 0.5.0 |
| HTTP 与 CDP 传输 | `reqwest` 0.12.28 → 0.13.5；`ureq` 2.12.1 → 3.4.2；`tokio-tungstenite` 0.26.2 → 0.30.0 |
| Stealth 传输 | `wreq` 6.0.0-rc.29 → 6.0.0-rc.31；`wreq-util` 3.0.0-rc.12 → 3.0.0-rc.14 |
| 字体与 SVG 渲染 | `swash` 0.2.0 → 0.2.10；`resvg` 和 `usvg` 0.47.0 → 0.48.1 |
| WebCrypto 哈希与 MAC | `sha1` 0.10.6 → 0.11.0；`sha2` 0.10.9 → 0.11.0；`hmac` 0.12.1 → 0.13.0 |
| WebCrypto 加密 | `aes` 0.8.4 → 0.9.3；`aes-gcm` 0.10.3 → 0.11.1；`cbc` 0.1.2 → 0.2.1；`ctr` 0.9.2 → 0.10.1 |
| 密钥派生与随机数 | `pbkdf2` 0.12.2 → 0.13.0；`hkdf` 0.12.4 → 0.13.0；`getrandom` 0.2.17 → 0.4.3 |
| 通用工具 | `base64` 0.22.1 → 0.23.1；`thiserror` 2.0.18 → 2.0.21；库 crate 也从 `thiserror` 1.x 迁移至 2.x |
| TLS 测试支持 | `rcgen` 0.13.2 → 0.14.10；`tokio-rustls` 0.26.4 → 0.26.6 |
| 原生构建支持 | `cc` 1.2.64 → 1.7.0，同时新增为 JavaScript 运行时的直接构建依赖 |

`deno_core` 仍为 0.412.0，新增浏览器 realm 补丁；保留原有带补丁的
`cosmic-text` 0.14.2 和 `taffy` 0.12.1。部分工具的旧版本仍作为间接依赖存在。

来源：[Cargo.lock](Cargo.lock)、[工作区依赖](Cargo.toml)，以及
[JavaScript](crates/obscura-js/Cargo.toml)、[网络](crates/obscura-net/Cargo.toml)、
[渲染器](crates/obscura-render/Cargo.toml) 和 [库](crates/obscura/Cargo.toml) 的依赖声明。
仓库内补丁说明见 [从源码构建](docs/Build-from-source.md#fork-dependency-patches)。

## 从源码构建

需要当前改动时，请从本仓库构建。项目名称是 Obscura-Hunt，命令和可执行文件仍叫
`obscura` 与 `obscura-worker`。

### 构建要求

- 当前稳定版 Rust 和 C/C++ 工具链。
- stealth 构建需要 CMake、Clang 和 libclang/LLVM 开发库。
- Windows x64 需要 Visual Studio C++ Build Tools、CMake、LLVM/Clang，以及 `PATH` 上的 NASM。
  构建前必须将 `LIBCLANG_PATH` 指向实际包含 `libclang.dll` 的 LLVM 目录。
  已安装 Clang 或 `PATH` 上有 `clang.exe`，不代表 bindgen 能找到该 DLL。
  详见[Windows 构建说明](docs/Build-from-source.md)。

Ubuntu/Debian 可这样安装原生工具：

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

### 构建渲染与 stealth 版本

```bash
git clone https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt

# 推荐使用编译缓存；已经安装时跳过这一步。
cargo install sccache --locked
```

POSIX shell：

```bash
export RUSTC_WRAPPER=sccache
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

PowerShell：

```powershell
$env:RUSTC_WRAPPER = "sccache"
# 改为自己的 LLVM 安装目录，其中必须包含 libclang.dll。
$env:LIBCLANG_PATH = "C:/Program Files/LLVM/bin"
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

默认构建产物位于 `target/release`（Windows 为 `.exe`）。复制到其他目录时，也要将同一构建的
`obscura` 与 `obscura-worker` 放在一起。并行抓取、多 worker 服务等命令需要 worker。
将该目录加入 `PATH`，或在命令和 MCP 客户端配置中使用可执行文件的完整路径。

Cargo 自动使用本分支随仓库提供的依赖补丁，构建时请保留它们。
补丁说明和更多平台指导见 [从源码构建](docs/Build-from-source.md)。
`sccache` 不能代替原生构建工具。

只需要渲染、不需要 stealth 时，使用 `--features render`。
不需要渲染时，使用 `--no-default-features`，可再加 `--features stealth`；
每种构建都保留 `--release -p obscura-cli --bins`。
无渲染构建不提供截图、screencast 或 PDF 输出。

### 下载发布包

在 [v0.1.2 Release 页面](https://github.com/MythHack/obscura-hunt/releases/tag/v0.1.2)下载发布包。
原生构建覆盖 Linux x64/ARM64、macOS x64/ARM64 和 Windows x64。
默认包启用渲染，`-stealth` 启用渲染与 stealth；另提供 `-no-render` 和
`-no-render-stealth`。每个包包含同一构建的 `obscura`/`obscura-worker`、
`LICENSE` 和 `build-info.json`。先用 `SHA256SUMS.txt` 核对压缩包，
再运行 `obscura --build-info` 查看实际源码提交与编译 features。
上游发布包和 Docker 镜像不包含本分支的改动。

## 使用方法

以下示例假设本分支可执行文件已经加入 `PATH`。

### 读取页面或截图

```bash
obscura fetch https://example.com --eval "document.title"
obscura fetch https://example.com --dump links
obscura fetch https://example.com --dump text --output page.txt
obscura fetch https://example.com --screenshot page.png
```

截图需要渲染功能，由原生引擎生成，不会启动 Chromium。

### 连接 Puppeteer 或 Playwright

```bash
obscura serve --port 9222

# 已编译 stealth 时，在运行时启用。
obscura serve --port 9222 --stealth
```

Puppeteer 连接 `ws://127.0.0.1:9222/devtools/browser`。
Playwright 的 `chromium.connectOverCDP` 连接 `ws://127.0.0.1:9222`。
可用示例见 [Puppeteer 指南](docs/Use-with-Puppeteer.md) 和
[Playwright 指南](docs/Use-with-Playwright.md)。

### 同时抓取多个页面

```bash
obscura scrape https://example.com https://news.ycombinator.com --concurrency 2 --format json
```

将 `obscura-worker` 保持在 `obscura` 同目录。

### 使用 MCP 客户端

```bash
# stdio：供启动子进程的客户端使用
obscura mcp

# HTTP：端点 http://127.0.0.1:8080/mcp
obscura mcp --http --port 8080
```

先用 `browser_navigate` 导航，通过 `browser_snapshot` 读取页面，再用 `browser_click`、
`browser_fill` 等工具操作。执行异步 JavaScript 时，`browser_evaluate` 支持 `awaitPromise: true`。
等待稍后才会满足的条件时，使用 `browser_wait_for_function`；其 `timeout` 和 `pollIntervalMs`
以毫秒为单位，条件未满足时返回超时错误。

启用渲染的构建可通过 MCP 输出静态截图和 PDF；流式 screencast 使用 CDP。
客户端配置、工具和 HTTP 认证说明见 [MCP 指南](docs/Use-the-MCP-server.md)。
非 loopback 的 HTTP 绑定需要认证。如果安装了其他 Obscura，请明确选择本分支可执行文件。

### 访问自己的本机开发服务

默认阻止私有/内部网络请求。访问自己控制的服务时，显式启用：

```bash
obscura fetch http://127.0.0.1:3000 --allow-private-network --dump text
obscura serve --port 9222 --allow-private-network
```

完整网络规则见 [环境变量说明](docs/Environment-variables.md)。

## 本地基准（2026-10-11）

以下是本机实测，不是上游发布的 Linux/无渲染成绩。机器为 Windows x64（10.0.26300）、
Intel Core Ultra 7 265K、20 个逻辑 CPU、47.28 GiB 可用内存、Python 3.13.12。
两个 Obscura 均使用 release `render,stealth` 构建；全部测量页面都是 loopback fixture，
不访问真实公网目标。使用[官方 obscura-benchmark 的
`2d7bc304` 版本](https://github.com/h4ckf0r0day/obscura-benchmark/tree/2d7bc304fbb7ebefc44f770938192e3cede87dbd)，
并应用仓库提供的 [Windows 测量补丁](render-repros/benchmark-windows.patch)。

### 能力与 stealth

官方 Obscura 来自
[`2bda71a`](https://github.com/h4ckf0r0day/obscura/commit/2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8)
（`0.1.0-dev+2bda71a`）；Obscura-Hunt 来自
[`2816993`](https://github.com/MythHack/obscura-hunt/commit/2816993483232d99495a28e7ba946713d0035481)。
本分支可执行文件报告 `0.1.1-dev+f9624e3`，版本后缀与记录的源码提交不同，
因此请结合源码提交与下方二进制哈希确认来源。

| 断言套件 | 官方 Obscura | Obscura-Hunt |
| --- | ---: | ---: |
| Obstacle course | 32/33 | 32/33 |
| Stealth（`--stealth`） | 89/112（79.5%） | 88/112（78.6%） |

两个 obstacle course 都只有 `fingerprint` 失败：已设置 `OBSCURA_TIMEZONE=Europe/Berlin`，
但期望 `Europe/Berlin`，实际得到 `Asia/Shanghai`。
23 个共同的 stealth 失败包括可枚举的引擎全局变量、两个空 WebGL vendor/renderer 值、
heap limit 不匹配，以及 19 个 Linux UA/platform 预期与当前 Windows 构建不符。
唯一变化的判定是 `fp_device_pixel_ratio_consistent`：官方的期望/实际为 **1/1**，
本分支为 **2/1**。套件按屏幕宽度 ≥2560 推断 DPR 应为 2，否则为 1；
不同的期望值体现 profile/启发式的限制，不能证明浏览器回归或真实反自动化检测结果。
两个能力 runner 都因失败断言退出 1；两个 stealth runner 虽有失败断言，仍退出 0。

### 冷进程 DOM 输出比较

Chromium 使用 Playwright 的 `chromium-1223`，SHA-256 为
`290fa7018fda22c52ada5eddb0113baf3ebc41fd0fde6085eddb19793606c635`。

| 测量项 | 官方 Obscura | Chromium 批次 1（配对官方） | Obscura-Hunt | Chromium 批次 2（配对本分支） |
| --- | ---: | ---: | ---: | ---: |
| 33 页各自冷进程耗时中位数的中位数 | 3,098.4 ms | 439.3 ms | 3,186.7 ms | 754.0 ms |
| 33 页各自进程树 RSS 采样峰值中位数的中位数 | 28.2 MiB | 414.8 MiB | 29.4 MiB | 463.1 MiB |
| 有效 / 被筛除的计时 DOM 输出样本（共 99） | 99 / 0 | 99 / 0 | 99 / 0 | 99 / 0 |

两个 Chromium 列是同一可执行文件的独立实测批次，没有合并样本。
参考浏览器自身的结果也随批次变化，因此不能用这两次运行隔离本分支改动的影响。
每列有 33 次预热和 99 次计时；全部 528 次尝试中，超时、非零退出、RSS 采样错误、
RSS 缺失均为 **0**。两个 comparison runner 都退出 0。

以下四页的 DOM 输出最大记录长度不同（字符数）：

| 页面 | 官方 Obscura | Chromium 批次 1 | Obscura-Hunt | Chromium 批次 2 |
| --- | ---: | ---: | ---: | ---: |
| `ssr-hydrate` | 2,157 | 2,151 | 2,157 | 2,151 |
| `es-modules` | 1,344 | 1,341 | 1,344 | 1,341 |
| `observer-intersection` | 3,599 | 2,359 | 3,599 | 2,669 |
| `spa-mini-app` | 3,771 | 3,768 | 3,771 | 3,768 |

仅凭长度不能证明 DOM 内容或画面等价。

### 比较实际测量什么

- 33 个页面，每个引擎每页预热一次、计时三次，两次运行之间间隔 100 ms，
  进程超时为 60 s。每次启动冷进程；Chromium 使用隔离的临时 incognito profile，
  不复用浏览器或 tab。
- 耗时包含进程启动、加载、JavaScript 和 DOM 序列化
  （`fetch --dump html` 对 `--headless --dump-dom`），不验证像素或布局正确性。
  每页取有效计时样本的中位数；汇总耗时和内存再取 33 个页面中位数的中位数，
  不是吞吐量或并发测试。
- Obscura 等待 **3 秒真实时间**，Chromium 使用 **3,000 ms 虚拟时间预算**，
  二者不是等价的墙钟等待策略。两个批次中 Chromium 耗时更短、Obscura RSS 更低，
  不能据此宣称通用引擎速度优势。逐页 Chromium/Obscura 耗时比的中位数分别为
  0.14（官方）、0.24（本分支），RSS 比为 14.48、15.09；比值的中位数不等于汇总中位数之比。
- Windows RSS 每 30 ms 采样一次根进程与所有子孙进程驻留内存的总和，
  取采样最大值后按中位数汇总。可能漏掉更短的尖峰，不等于 GNU `time -v`
  的单进程峰值 RSS、分配总量或稳态内存。
- 沿用原有非空/输出长度启发式筛选样本，它既不是 DOM 语义断言，也不按退出码判定有效。
  退出码、超时和 RSS 错误单独报告。只有 obstacle course 检查 manifest 中的预期值/内容，
  使用最后一次计时输出判断（预热一次、计时五次）。
- Chromium 请求 1280×720、device scale 1，未检查实际 CSS viewport。
  Obscura 指定 profile 0、`Europe/Berlin`，禁用 profile 轮换；
  只有 stealth 套件在运行时启用 stealth。
- 本次没有测量 Firefox、Chromium 能力/stealth 分数、WPT、截图、生产环境 tab 复用、
  公网页面成功率或 Cloudflare 绕过率。

### 在 Windows 上复现

按上文 release 命令构建浏览器源码版本 `2816993`，然后从本分支根目录开始。
两个 Obscura 批次使用同一个 Chromium 可执行文件；更换构建会改变比较条件。
使用 `CARGO_TARGET_DIR` 或其他 Chromium 安装位置时，请调整可执行文件路径。
对每个 Obscura 可执行文件分别运行三个套件，保存独立 JSON，并记录源码版本、
`obscura --version`、可执行文件/worker SHA-256 和 Chromium SHA-256。

```powershell
$fork = (Get-Location).Path
$suite = Join-Path (Split-Path $fork) "obscura-benchmark"
git clone https://github.com/h4ckf0r0day/obscura-benchmark.git $suite
git -C $suite checkout 2d7bc304fbb7ebefc44f770938192e3cede87dbd
git -C $suite apply "$fork/render-repros/benchmark-windows.patch"
python -m pip install psutil

$env:OBSCURA_BIN = "$fork/target/release/obscura.exe"
$env:CHROME_BIN = "$env:LOCALAPPDATA/ms-playwright/chromium-1223/chrome-win64/chrome.exe"
$env:OBSCURA_ALLOW_PRIVATE_NETWORK = "1"
$env:OBSCURA_PROFILE = "0"
$env:OBSCURA_TIMEZONE = "Europe/Berlin"
$env:PYTHONUTF8 = "1"
Remove-Item Env:OBSCURA_ROTATE_PROFILE -ErrorAction SilentlyContinue
Push-Location $suite
python obstacle-course/run.py --json | Set-Content -Encoding utf8 fork-final-capability.json
python stealth-bench/run.py --stealth --json | Set-Content -Encoding utf8 fork-final-stealth.json
python compare/head-to-head.py --json --runs 3 --warmup 1 --wait 3 --label fork-final-vs-chromium --output fork-final-comparison.json
Pop-Location
```

官方基线单独克隆固定版本并构建；将匹配的 worker 保持在可执行文件同目录：

```powershell
$official = Join-Path (Split-Path $fork) "obscura-official"
git clone https://github.com/h4ckf0r0day/obscura.git $official
git -C $official checkout 2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8
Push-Location $official
cargo build --release --locked -p obscura-cli --bins --features render,stealth
Pop-Location
$env:OBSCURA_BIN = "$official/target/release/obscura.exe"
Push-Location $suite
python obstacle-course/run.py --json | Set-Content -Encoding utf8 official-2bda71a-capability.json
python stealth-bench/run.py --stealth --json | Set-Content -Encoding utf8 official-2bda71a-stealth.json
python compare/head-to-head.py --json --runs 3 --warmup 1 --wait 3 --label official-2bda71a-vs-chromium --output official-2bda71a-comparison.json
Pop-Location
```

### 证据与二进制来源

| SHA-256 | 官方 Obscura | Obscura-Hunt |
| --- | --- | --- |
| `obscura.exe` | `5b59abfb535b8d0784787b6fa09b9d64cafd4a0f4a41e3a3f533869ceab306ae` | `aa18963fcfda4dc568eae40f7f220fad27906460cc43c173bc5e47c9b0d425fa` |
| `obscura-worker.exe` | `787126df746bd27caf4218514544329d17d7e2dd2e4dd71edd9e4adeb22161e8` | `230137e21e7a2c74c6cfd67bcf34ebde9096396a99ecc57334934a5735a3410f` |

两个批次使用相同的打补丁后 `compare/head-to-head.py`，SHA-256 为
`92ba3bbea5367145af3b6a41b7362549c26160037c0d3ec153d236180d801424`。
本地证据文件为 `official-2bda71a-{metadata,capability,stealth,comparison}.json`
和 `fork-final-{metadata,capability,stealth,comparison}.json`，位于
`D:/code/.bench-results/20261011-local/`；metadata 记录命令、可执行文件/worker
哈希及所有 runner/manifest 哈希。comparison label 分别为
`official-2bda71a-vs-chromium`（批次 1）与 `fork-final-vs-chromium`（批次 2）。
这些本地文件不是上游发布的结果。

## 已知限制

- 当前构建没有音视频编解码与播放管线。媒体属性和字幕轨道可用，但不支持的源会失败，
  不会伪造媒体元数据或播放成功。
- 修正浏览器行为可以消除具体的不一致，不等于与 Chromium 完全不可区分。
  stealth 不保证绕过 Cloudflare 或其他反自动化系统，本地基准分数也不是验证码通过率。

## 更多指南

- [CLI 参数参考](docs/CLI-reference.md)
- [Cookie 与存储](docs/Persist-cookies-and-storage.md)
- [从源码构建](docs/Build-from-source.md)

## 署名与许可证

Obscura-Hunt 基于 [h4ckf0r0day/obscura](https://github.com/h4ckf0r0day/obscura)。
保留上游贡献者署名、版权和许可证声明。
采用 [Apache License 2.0](LICENSE)。
