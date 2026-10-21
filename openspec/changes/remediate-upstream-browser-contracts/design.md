# Design

## Context

动机与能力边界见 [proposal](proposal.md)。原调查与实施基线记录为 `208608215a42f2c392b13ccce62f433a4441155a`，当时已合入上游 `a754dce`；这些是历史定位信息，不代表当前源码或产物状态。仓库此前没有 OpenSpec，当前 10 个新增规范文件是契约的首次记录，不表示要重写已有浏览器。

[完整调查快照](evidence/triage-report.json)含当时的 102 个开放 Issue：20 项已有源码/历史覆盖、27 项部分覆盖、34 项修复/能力候选、14 项诊断、6 项可选、1 项不采纳。最初的 16 项是混合 Issue/PR 列表的首批结果，已明确纠正。102 项正文均已分类，但未审计全部历史关闭 Issue、全部评论或整个源代码，也没有把 102 项全部重新运行；快照数量与分类不改写成当前状态。

实际所有权是页面/连接所属的原生 V8 线程与 realm、frame/document-generation-scoped 节点。部分旧架构文档的“全局单 isolate/全局 V8 锁”不代表当前实现。以 `crates/obscura-cdp/src/server.rs`、`dispatch.rs` 和当前 runtime 的实际边界为准。

## Goals / Non-Goals

**Goals:**

- 全部 26 个 RC 保留唯一主归属，合并为 1.1–10.1 十个交付任务；每项同时覆盖最早的信息丢失点、所有生产消费者、旧路径移除与必要行为验收，11.1 统一集成验证与交付，不以重新编号增加通过数。
- 每个开放 Issue 恰有一个主处置，复合 Issue 的其他责任显式关联。例如 #1295 的解码与 CSS、#855 的路由与 Cookie 权限，不因分组丢项。
- 保留真实浏览器行为、原生 fast path、panic-safe ops、结构化克隆、来源/权限、原生 frame/WindowProxy 和取消状态机。
- 完整公钥能力与完整区域配置不缩为 API 存在、伪密钥、硬编码 locale 或伪造 RTC 成功。
- 覆盖报告中的确认缺口和明确剩余契约；未知历史调查保留有界登记，不自动阻塞交付；具体当前复现和改动触及的不安全路径不能借此移出责任。

**Non-Goals:**

- 不整批盲合或重写可复用实现：对应上游 PR 代码与已有测试是首选，只在本分支实际冲突或确实缺行为处修改，不为证明接线另建框架。
- 不将 20 个已有覆盖项重新列成 20 个生产修复，也不把源码有函数当成本次验收已通过。
- 不自动新增 Chrome 扩展宿主、SIXEL UI、第三方搜索集成、包管理渠道、legacy CBC 默认回退或未经授权的存储重构。逐项见 [scope-decisions](scope-decisions.md)。
- 不新增站点特判、伪全局、异常吞掉、挑战答案工作流、盲目重试、以加长超时代替状态机修复；不禁用 watchdog。
- 不以更大的 JPEG 冒充 PDF 文字层，不以字符串 ICE 地址替代真实可达的传输。GeoIP 地址契约包含真实 ICE/DataChannel 对照；完整音视频采集/编解码平台不是本 Issue 的要求。
- 不建立客户端×平台×代理的完整产品矩阵，不强制小时级 profile 或新消费者框架，不因缺少 build.log 创建归档或重编旧版本；只保留真实功能契约、回归边界和仓库必需验证。

## Decisions

### 1. 按拥有边界收敛，不建立总括框架

26 个 RC 单元描述首个分歧点，不是 26 个独立交付任务或新模块。交付按真实拥有边界收敛如下；同一行中的 RC 不丢失各自功能、拒绝与生命周期契约。

| 交付任务 | 拥有边界 | RC 唯一主归属 |
| --- | --- | --- |
| 1.1 | 请求决策、原生传输与网络事实 | RC-01、RC-02、RC-03、RC-04、RC-07 |
| 2.1 | TLS 缺链恢复与信任边界 | RC-05 |
| 3.1 | CDP 目标、会话与 DOM 发布所有权 | RC-06、RC-08、RC-09 |
| 4.1 | 文档、Worker 与异步任务生命周期 | RC-10、RC-11、RC-12、RC-15 |
| 5.1 | XML 解析与流式解码 | RC-13、RC-14 |
| 6.1 | 真实 WebSocket 连接生命周期 | RC-16 |
| 7.1 | 原生公钥 WebCrypto | RC-17 |
| 8.1 | 区域配置、权限与真实 RTC 路由 | RC-18 |
| 9.1 | 共享级联、布局、字体与绘制结果 | RC-19、RC-20、RC-21、RC-22、RC-23、RC-24、RC-25 |
| 10.1 | 已有 profile 的提交、隔离与权限 | RC-26 |
| 11.1 | 统一集成验证与交付 | 共享收口，不重复取得 RC 主归属 |

先复用对应上游 PR 的适用实现和已有测试，再按当前分支的原生 realm/线程所有权、frame/document-scoped ID、SSRF/TLS/代理与取消边界解决冲突。上游已合入的正确行为保留，不重复移植；确实缺失的行为才新增实现。根因分组不要求新增请求、任务或消费者框架。若公共字节 API 必须更改，迁移全部调用者和文档后删除旧字符串入口，不用别名维持错误语义。

每个交付任务在同一逻辑改动中连接生产者、序列化、全部受影响消费者、失效清理和必要回归；不再拆为一份实现勾选加一份验收勾选。任务按所属 Requirement 保留可观察结果，11.1 复用这些结果并集中运行共享验证，不能用 helper/API 存在或任务压缩代替真实功能交付。消费者和平台按实际受影响路径选取，不为每个 RC 重跑同一套组合。

### 2. 请求策略在发送前决定，事件来自实际请求

复用共享来源、referrer、deny-set、代理和客户端选择。文件授权在 metadata/read 前检查，CIDR 只放开显式允许的被拒绝地址；URL、DNS 和每跳 redirect 一致，不能以另一个无防护客户端补救。

fetch/XHR/导航/脚本/表单/模块/本地资源有不同浏览器算法，不能仅因共用 HTTP 就强行同一化。相同策略复用，不同请求模式、同步性与凭据规则保留。Fetch Metadata 的 mode 取真实 request mode；opaque origin 不默认等于直接用户导航的 `none`；完整 redirect 链和可信 URL 条件必须保留。

Fetch 拦截必须在 I/O 前产生决策通道。网络完成后补发 `requestPaused` 不是实现。二进制请求和响应保持 bytes、view offsets、MIME、revocation 与错误时 pause 所有权。生产事件包含实际发送 headers/body/ExtraInfo、发生时点和最终响应方法，307/308 不得一律记 GET。

TLS AIA 只能在受限证书材料范围内恢复缺失链，再进行正常完整验证；候选中间证书不是信任根。拒绝验证关闭的应用 HEAD 探测、直连回退和无限递归/缓存。保留代理、SSRF、大小/深度/时间界限。

### 3. 目标身份、执行所有者、会话订阅分离

浏览器目标可跨连接发现和附着，但真实 Page/V8 仍在原 owner 线程执行。共享的是稳定目标元数据和发往 owner 的有界命令，不是把 V8 对象移到任意线程，也不是只公布不可用的 targetId。

HTTP discovery、浏览器 WebSocket、页面 WebSocket、managed/flattened session 使用同一个真实目标身份。每会话的启用/禁用/分离只释放自己的订阅；响应体缓存的存活不能被一个旁观会话提前结束。

DOM 实时事件沿已发布的 scoped ID、文档代次和会话树发送；保留已合入的四个查询方法，新增事件而不恢复旧 JS 对象表。尚不支持的 piercing 路径明确报错，不宣称因此完整实现 shadow/frame 发布。

### 4. 不可丢失的导航提交不能放进短时可取消轮询

MCP selector/text/function wait 共用一致的推进/提交原则，但不同等待条件不必抽成新框架。轮询 timeout 只取消等待本身，不消费后丢失 pending navigation。新 URL、DOM、文档代次和生命周期事件必须一起提交或保留明确失败。

CDP 在导航期间接受输入不等于可以延迟所有回复，也不等于可以成功确认后丢弃事件。保留对应文档、取消、关闭和输入默认行为；同步失控 JS 由 V8 watchdog 和进程 deadline 截断，而不是让网络 timeout 代替不可抢占的保护。

### 5. DOM、Worker 与资源有真正的所属环境

`window.document` 属性约束由正确的 Window 绑定承担，不包装任意站点脚本。javascript iframe URL 在目标 child Window 求值，字符串 completion 创建对应 child Document；它不是允许一个新网络 scheme。

Worker 具有自己的真实内建对象与同步 importScripts 环境，不回退到宿主页面全局。MessagePort/ArrayBuffer 转移遵循结构化克隆、端口/缓冲所有权、来源和终止边界。

XML 使用真实 XML 节点、命名空间和文档所有权；保留 HTML 与 parsererror 路径。编码器保留跨 chunk 状态。本地 data/blob 在加载边界处理真实字节/MIME/撤销，不能绕过外部网络或文件权限。同步 XHR 在 `send` 返回前完成，不以异步请求或伪 status 实现。

页面 WebSocket 的 OPEN、消息和关闭由真实 ws/wss 帧与连接驱动。CDP 服务器用了 WebSocket 库不代表页面客户端已经实现；代理、网络权限、Origin、协议协商、二进制及取消均要在公开调用路径验收。

### 6. 布局/绘制/几何/输入共享真实数据

修复 display/percentage/baseline/inline-owner 信息首次丢失的位置，随后同时验证 paint、DOM rect、visibility 和 CDP hit targeting。不要通过改返回值制造看似正确的几何。控件要求值净化、默认动作、取消、可信事件和实际绘制闭环。

UA 默认值进入正常级联，typed custom properties 和动态选择器进入查询与样式状态，而非独立硬编码查询。计算值、used value 和绘制效果按当前规范分开；过时的本地措辞/实现断言应移除，不能为保留旧测试而偏离浏览器契约。

Canvas 颜色与像素、SVG 长度、字体 measurement/paint、prepared/fallback capture 都使用对应真实数据。PDF 从布局/绘制输出矢量、文本和嵌入字体，迁移 CDP、MCP 和 Rust 消费者，不保留只会 raster 的旧公开入口。允许真正图像内容栅格化，不把全部文字转为背景图。

### 7. WebCrypto 使用真实原生密钥和算法能力

优先评估已有 `aws-lc-rs`，但库存在并不证明各算法/曲线/格式已覆盖。七类算法分别完成适用的生成、格式往返、加解密、签名验签、协商、wrap/unwrap 和拒绝行为。未知格式、错误 key type/usages/hash/curve、非 extractable 密钥不可通过兼容分支放行。

使用规范向量和独立浏览器互操作；ECDSA 签名格式、RSA-OAEP label/hash、RSA-PSS saltLength、ECDH/X25519 deriveBits 等不能只做同库自洽往返。原生 key material 不无谓复制或经普通 JS JSON 暴露，释放和失败路径同样受控。

### 8. GeoIP 是区域配置解析，不是权限或网络地址伪造

默认关闭时不探测 IP、不下载数据库。显式 IP 模式无需公网出口探测；自动模式通过当前有效代理/网络路线发现出口，不能把代理主机地址当出口，代理失败不能直连泄露真实地址。

优先级为支持的显式 scoped override、显式启动/API/环境配置、启用的 GeoIP 推导、默认值。解析一次并在稳定 profile 中按区域语言人口权重选择语言；不要每次导航重抽。维护国家/locale、坐标及准确度、时区、navigator、Accept-Language 和 native Intl 的一致来源。

优先本地只读 MMDB 查询，数据库缓存与出口缓存分开，记录来源、版本、许可与精度。更新需校验并原子替换；离线显式 IP+本地数据库可用。未知 IP/缺字段按明确配置错误处理，不静默换宿主身份或随机语言。禁止请求页面借此任意操纵数据库下载或探测地址。

ICU 时区/locale 的进程级机制不能被伪装为 per-context 隔离。采用真实 scoped 支持或 profile/进程归属；共享进程不支持的冲突必须明确拒绝，不能在已经运行的 isolate 间更改环境造成串扰。已有正确 frame/popup/navigation 传播路径保留。

位置权限独立于坐标配置：query、get/watch、origin/context 范围和 CDP grant/reset 一致。配置 GeoIP 不自动授予权限。

RTC 地址行为须有真实 ICE 与 DataChannel 连接，对候选、SDP、相关地址和 stats 以及 STUN/TURN 流量同时验收。HTTP 代理不天然承载 RTC；不支持所需隐私路由时明确失败，不能偷偷直连。此错误分支可以正确，但只有错误分支不算完成 GeoIP 的 RTC 能力。完整媒体采集/编解码不是当前目标。

### 9. 既有 profile 完成持久化闭环，不偷换存储架构

修复 cookie/localStorage 的现有导航和干净关闭提交，迁移受影响的 CLI、MCP stdio/HTTP 调用路径；平台私有权限差异在对应原生平台检查，不扩展为所有消费者×平台的产品矩阵。写入采用原子替换与平台适当私有权限；没有 tracing subscriber 时也不能静默丢失。失败必须到达真实调用者或明确持久化错误结果。

当前内存 IndexedDB 的事务、typed keys、structured clone 继续保持。持久 IndexedDB/CacheStorage 和 scrape 新 profile 协议按 [scope-decisions](scope-decisions.md) 另案，不用旧 PR 直接替换已正确的语义。

### 10. 有界调查与明确处置

[调查登记](investigations.md)保留原先 14 项诊断与 5 项 partial 条目的历史材料、缺失证据和未知结论，不再作为 19 项必过任务。仅未知历史报告不阻塞 11.1，也不要求为归因另建平台、长时 profile 或消费者框架；发现具体当前复现或本次改动触及不安全路径时，由对应交付任务处理最早分歧并保留最小行为证据。已确认的 INV-1151 classic fetch admission 与 INV-531 watchdog 边界归 4.1；INV-866 的具体 CDP timeout/context 问题由 3.1 负责会话/回复、4.1 负责文档与异步生命周期，不得退回“待调查”而丢失已知反例。仍未知部分继续可见，不能将局部修复说成整个上游 Issue 已解决。

可选/不采纳/新增部分另案，以及20项已有覆盖的保护映射，在 [traceability](traceability.md) 和 [scope decisions](scope-decisions.md) 可逐项追溯。所有102项都有主处置，不要求102个独立补丁。

## Risks / Trade-offs

- **共享文件冲突**：`bootstrap.js`、`ops.rs`、`runtime.rs`、`page.rs`、CDP server 和 MCP lib 是共享热点。一个集成 owner 管理这些文件的最终落地；可以并行编写独立规范/夹具，不能让多个实施者无协调重写同一区块。
- **安全与兼容同时成立**：AIA、本地 scheme、同步 XHR、WebSocket、GeoIP/RTC 都可能引入旁路。所有实际发送边界都复用现有权限；不以放宽验证“修兼容”。
- **性能**：避免多余 clone/编码/全树扫描/每页数据库读取；按相同 release/features、页面、网络、viewport、settle、capture 路径比较分布和进程树内存。约10%噪声不当作收益。无等价结果不接受性能数字。
- **规范变化**：当前标准与 Chromium 或旧测试有差异时记录具体版本和可观察值，选择有依据的浏览器契约；不能只重钉旧实现细节断言。
- **证据归属**：[历史观察](evidence/local-observations.json)没有记录执行当时二进制 SHA，只能定位缺口，不能用于给新任务打勾。源调查也不等于完整审计。

## Migration Plan

1. 固化基线、102项集合、已覆盖保护和不纳入边界，生成任务/Requirement/Issue/PR 双向关联。
2. 按上表 1.1–10.1 和任务书中的真实依赖执行，不以编号强迫无关串行；涉及相同生产文件时由集成 owner 排队，先移植适用上游实现、复用已有回归，再最小化修复本分支冲突或剩余行为。
3. 每个交付任务复用已知失败及现有测试/消费者，完成生产闭环、删除失效旧路径/断言、更新相应公开文档；仅真实回归或缺失边界需要新增测试，不为接线或存在性建立自证脚本。
4. 11.1 统一集成验证与交付：取得必需契约对应产物的行为结果并集中执行仓库验证。修复后只重跑受影响检查，复用与未受影响源码和产物适用的有效证据；未知历史调查单独保留，不设额外基线复选框，不把旧证据或重排任务当成新增通过。

## Validation Strategy

- 所有共享检查由 11.1 统一调度一次，不在十个交付任务或 26 个 RC 中各复制一轮。记录源码 revision/dirty 状态、实际二进制 SHA-256、已有 `--build-info`、平台/features、命令、fixture、退出码和行为结果；既有日志/结果足以定位时直接复用。缺失 build.log 不构成重编、归档或另造证据框架的理由；失败与尚未覆盖的必需契约保持可见。
- 保留仓库要求的 focused release `cargo nextest`、完整 `cargo nextest run --release --features render --no-fail-fast`；禁止 `cargo test` 和全树 `cargo fmt`。已有测试优先，仅真实回归或必要拒绝/生命周期边界未覆盖时新增测试，不改旧断言来迁就错误语义。
- 保留精确 release CLI/worker 构建（`CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render`）和离线 obstacle course 33/33；stealth 改动须用真实 `render,stealth` 路径，不能拿 render 结果代替。修复后重跑受影响检查，不让一次局部修改无条件重启整套验证。
- 仅对变更触及的 no-render、Windows/Linux 原生配置、密码学、权限或持久化差异选择对应验证；不要求所有客户端、平台、代理与 features 的笛卡尔积。真实支持的消费者仍须接通，不能把减少重复组合当作删减功能。
- 优先复用已有 CLI、CDP WebSocket、MCP stdio/HTTP、Puppeteer/Playwright 和 Rust 消费者及 PR 测试，按受影响公开路径取得最小充分证据，不另建通用消费者框架；内部测试不能独自替代尚未接通的公开运行表面。
- 渲染仍按仓库现有 deterministic fixtures、representative top/bottom 与 matched Chromium 方法；先确认导航成功和图像非空，再检查几何/资源/字体/裁剪/固定布局，不把像素距离当单独正确性证明。这些共享验证归 11.1，不按各绘制 RC 重复。
- 使用已有 WebCrypto/DOM WPT 时记录 subtest pass%，不报告整文件通过率。只有本次确实涉及性能边界或已有具体反例时，才按等价 release/features/页面/网络/viewport/settle/capture 对照工作量、时延与内存，不强制小时级 profile 或以不等价内容证明收益。
- 原规划曾记录的 OpenSpec strict validation、apply 只读解析和集合/引用检查仍是历史证据，不证明本次重排后已验证。11.1 统一核对当前任务和映射；保留不可变的历史观察与实施基线，不重复运行已报告失败来确认，也不以旧产物验收新源码。
