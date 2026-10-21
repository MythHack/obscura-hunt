# Tasks

## Execution policy

按用户最新要求，以现成 PR 为实现起点，按首个状态/信息分歧收敛。**10 个交付项 + 1 个统一收口项**；实现、调用方迁移、旧路径移除和必要验证属于同一交付，不再拆成两套复选框。26 个 RC 保留为行为契约和追溯标签，不是 26 个独立重写项目。

- **先复用，后补缺。** 查看已登记 PR 的实际 diff，优先移植其实现和有效测试；只解决本分支 native realm、owner thread、scoped ID、请求策略等真实冲突。没有可用 PR 或存在明确遗漏时才写最小补丁，不为统一外观重写有效实现。
- **一次修在拥有处。** 相同状态只有一个来源；生产者、调用方和协议序列化一起迁移，不保留损坏语义的别名、双轨实现、假成功或重试补救。不同浏览器算法不为“统一”而强行合并。
- **验证服务于缺陷。** 复用 PR/仓库已有行为测试，再选能观察根因修复的真实消费者。边界用例只针对会出错的权限、字节、状态转换或生命周期；一个场景覆盖多个条件即复用，不按客户端×平台×代理做无差别全组合。
- **停止条件明确。** 已知失败不再重跑确认；修复后行为正确、关键拒绝/取消边界保持、没有本次回归即可关闭该项。不默认追加长时 profiler、概率认证、另造发布包、全量 WPT 或新验收框架来反复证明。
- **保留真实性。** 不削弱 SSRF/TLS/来源校验、watchdog、原生线程/realm 所有权；不伪造 WebSocket、RTC、密钥、布局或 PDF 文字。实际改动涉及的普通/stealth、render/no-render 分歧必须各验证对应路径。
- **保留可追溯性。** [机器映射](traceability.json)与 [Issue/PR 映射](traceability.md)记录唯一 owner；[调查登记](investigations.md)保留未确认/缺材料项。只有当前可归因反例或受影响的安全/生命周期风险才升级为本轮阻塞；已确认的局部缺陷不能因调查分流而丢失。
- **不制造完成率。** 原基线记录继续有效，不把编号合并算作完成。每个交付通过最小充分实跑后立即同步复选框与 todo。证据记录实际产物 SHA、features、命令、退出码和关键观察；旧二进制或上游作者的结果不冒充本轮结果。

## Current state

十线源码与最终集成已收口，11/11 项实施任务完成。最终 CLI/worker 成对产物位于外部 `accepted-run/render-final-8162eb904332/`、`accepted-run/no-render-final-80de30935c79/`、`accepted-run/render-stealth-final-3e0d6c7d5f60/`；每目录 `provenance.json` 记录实际 SHA256、features 和构建结果。当前 `E:/rust/target/release/` 是最终 `render,stealth` 产物。汇总为 `accepted-run/delivery-final.json`，旧二进制与未受影响的消费者证据保留原始 hash，不冒充新版本重跑。

原 61 项失败已收口：删除 3 条无效契约测试，其余先通过 54 项、再只复查剩余 4 项且全部通过；随后全量 release/render 为 2552 通过、0 失败、5 跳过，测试执行 58.438 秒，报告为 `accepted-run/final-render-nextest-51f17ba4f981.xml`。该全量在最后 CSS/CDP fragment 修正前完成；后续只对实际受影响入口补验，没有重复全量或新增矩阵。

现有 CSS 消费者退出 0，最后 fragment 入口实跑也退出 0：不重载、不产生新 loader、保留 DOM 修改并更新 `:target`，实际截图确认。render、no-render、render/stealth 三种 CLI/worker 均构建成功；no-render 与 `--stealth` 启动/DOM 实跑通过。no-render 保留 8 条非阻断 dead_code 警告，未加 allow 掩盖，也不宣称零警告。此前 obstacle course 33/33 与 deterministic、broad top/bottom 证据按原始产物复用，本次不再重复；失败导航、不稳定图像不计作保真成功，不声称像素完全等同 Chromium。nextest 单项 120 秒上限和 JUnit 保留有效。外部产物根目录为 `D:/temp/obscura-remediation/accepted-run/`。

共享文件由主集成者唯一落地，Cargo 构建/nextest 集中执行。PR 关联表示可复用来源，不表示已经原样合入；缺陷已经修复的既有路径保持不动。

## 1. 请求决策、原生传输与网络事实

- [x] 1.1 合并请求身份、所选传输、发送前拦截、地址/文件权限与真实网络事件修复，并完成必要验证。

**根因 / 契约：** 请求身份、方法、字节和权限在进入不同发送路径时丢失，CDP 又从事后快照重建事实。一个真实请求在发送前完成策略和拦截决策，事件从该请求发布。覆盖 RC-01/02/03/04/07。

**优先复用：** PR [#1301](https://github.com/h4ckf0r0day/obscura/pull/1301)、[#1279](https://github.com/h4ckf0r0day/obscura/pull/1279)、[#981](https://github.com/h4ckf0r0day/obscura/pull/981)、[#644](https://github.com/h4ckf0r0day/obscura/pull/644)、[#718](https://github.com/h4ckf0r0day/obscura/pull/718)、[#1280](https://github.com/h4ckf0r0day/obscura/pull/1280)、[#1250](https://github.com/h4ckf0r0day/obscura/pull/1250)、[#1118](https://github.com/h4ckf0r0day/obscura/pull/1118)。缺少的 CIDR/调用方传播只在现有 net/ssrf/context 层补齐。

**收口：** net client/wreq、JS fetch/XHR/module、Page 导航/表单、CLI worker 与 CDP 共用原有策略；保留各自 mode/凭据/redirect 语义。移除字符串响应体和重复文档拦截，保留类型过滤及 raw bytes。改写后的 URL 和每跳 DNS/redirect 仍过权限门。

**充分验证：** 复用 network_pr_ports、fetch_modes、binary_fetch_body、file_navigation_gate；一个本地请求链核对身份、POST/重定向、暂停重写、原始响应体与事件。另验证 Fail/Fulfill 不发送、非法改写不放行。所选 transport 的普通/stealth 路径分别运行；公共入口只在不同传播代码确有变化时增加场景。依赖 3.1 的目标绑定用于完整 CDP 消费，不阻塞独立 net 修复。

**实跑证据：** `accepted-run/network-contract-completed/result.json` 的最后补充退出 0，普通/stealth 的断开和未回复暂停均有界失败且零目标 I/O；该文件明确标为 selected supplement，不冒充整套重跑。实际 CLI/worker SHA256 为 `988ccd2a92f050808181e28d293c06659ad8ee89d0a04bf57df967590d3f2957` / `93c496a3b7bff044fde148526855218073e61a1a3435e545a5654308e1d4430f`。复用 `network-closed/` 的 transport/policy、`network-closure/` 的 CLI/serve/scrape/MCP 身份与 image/interception、`network-font-final/` 两后端 Font Continue/rewrite/Fulfill/Fail，以及之前原有网络消费者的剩余通过结果。同步 XHR 复用 4.1；相关 network_pr_ports/fetch_modes/binary_fetch_body/file_navigation_gate 已在集中 focused 运行通过。

## 2. TLS 缺链恢复与信任边界

- [x] 2.1 合并受限证书补链，保留正常验证和有效代理路线。

**根因 / 契约：** 缺少中间证书不是放弃信任验证的理由。覆盖 RC-05；优先复用 PR [#1217](https://github.com/h4ckf0r0day/obscura/pull/1217)，只替换与本分支正常验证、代理、SSRF 冲突的部分。

**收口：** 在所选原生 TLS 层补链，再完整验证 hostname/有效期/用途/信任根；不发送无验证应用探测，不直连回退，不引入第二套宽松客户端。

**充分验证：** 复用 aia_recovery 和现有本地 CA 消费者：缺链成功、错误主机/不受信根失败、补链遵守代理与地址限制；普通/stealth 各覆盖受改动后端。大小/深度/超时边界复用已有测试，不重复全组合压力矩阵。依赖 1.1 的请求策略。

**实跑证据：** `accepted-run/aia-final-ordinary/receipt.json` 与 `aia-final-stealth/receipt.json` 均记录退出 0，实际 CLI SHA256 `41a7e3141e0da9ef41d04db712050dea365842b4688054c237d8bc37ca9b53fb`。普通与 stealth 分别通过缺链补齐、代理路线及主机名、有效期、信任根和受限 AIA 拒绝；复用这些结果，不再扩展证书组合。

## 3. CDP 目标、会话与 DOM 发布所有权

- [x] 3.1 合并全局目标绑定、会话隔离、精确控制面路由和实时 DOM 发布。

**根因 / 契约：** 浏览器目标身份、执行 owner 和会话订阅混在连接生命周期里，导致 No page、事件错投和旁观者删共享 body。覆盖 RC-06/08/09；PR [#829](https://github.com/h4ckf0r0day/obscura/pull/829)、[#686](https://github.com/h4ckf0r0day/obscura/pull/686) 为起点，已合入 [#1300](https://github.com/h4ckf0r0day/obscura/pull/1300) 的查询能力保持。

**收口：** discovery/browser/page endpoint 绑定同一真实目标，命令回到原 V8 线程；会话只释放自己的订阅。DOM mutation 使用既有 scoped 节点和文档代次。HTTP 按请求目标精确路由，保留 Host/Origin/auth 拒绝；构建日志从源码交付集合排除，不另造发布工程证明这一点。

**充分验证：** 复用 session_subscriptions/dom_node_queries 和现有原始 CDP 消费者：两连接附着同一页、一方 disable/断开后另一方继续取 body/操作；已发布 DOM 的增删和导航后旧 ID 拒绝；路由近似路径/头值不得误命中。沿既有 Playwright 或 Puppeteer 运行一条真实页面链，不要求每个客户端重跑同一矩阵。INV-866 已记录的超时后原 context 恢复由本项联合 4.1 修复验证。

**实跑证据：** `accepted-run/puppeteer-release-candidate/puppeteer-result.json` 的真实 Puppeteer 消费者退出 0，CLI SHA256 `46bfd23b027c6203631d318f3e864ee0f42a77ac6d15608acc0d5d952464417f`。复用 `cdp-integrated-delivery/` 已执行通过的 HTTP/raw CDP 路由、双连接 body 订阅、DOM publication/移除/导航代次、真实请求事件；不把它此前被 Puppeteer 默认 touch=false 拦住的整体退出码改写为成功。已修复该无触摸默认值的路由，启用触摸仍明确拒绝。popup debugger 三条 focused 回归与真实恢复后 load/postMessage 均通过；Document 与消息回调的 V8 存储槽冲突已消除，无新运行时或等待重试层。

## 4. 文档、Worker 与异步任务生命周期

- [x] 4.1 合并导航提交、Document/Worker 归属、本地资源与同步请求修复，消除已确认的脚本调度分歧。

**根因 / 契约：** 文档代次、资源终态和不可丢失提交未由正确 owner 持有，短轮询取消或 Worker/page 混用会丢状态。覆盖 RC-10/11/12/15；保留已合入 PR [#1298](https://github.com/h4ckf0r0day/obscura/pull/1298)，优先复用 [#1251](https://github.com/h4ckf0r0day/obscura/pull/1251)、[#1248](https://github.com/h4ckf0r0day/obscura/pull/1248)、[#1249](https://github.com/h4ckf0r0day/obscura/pull/1249)、[#1256](https://github.com/h4ckf0r0day/obscura/pull/1256)、[#909](https://github.com/h4ckf0r0day/obscura/pull/909)。

**收口：** 不可替换的公开 document、child javascript 导航、preload 终态、Worker 原生全局/importScripts/transfer、data/blob bytes 与撤销、同步 XHR 都在对应现有 owner 中实现。MCP wait 不丢导航提交，CDP 加载时真实输入可推进，取消不重建页面来掩盖状态丢失。INV-1151 的已确认 classic fetch admission 与 INV-531 的 phase watchdog 错配直接归入本项，不另建优化项目。

**充分验证：** 复用已准备的 DOM/Worker/XML 相邻消费者、navigation wait/CDP input 和 classic-fetch 场景；观察实际 Worker 消息/转移、同步 send 完成、文档替换后取消、慢合法脚本与失控脚本区分。网络权限复用 1.1 的同一拒绝证据，不另造 DNS/TLS 框架。依赖 1.1/3.1，解码部分复用 5.1。

**实跑证据：** `accepted-run/dom-contract-completed/evidence.json` 与 `worker-final-completed/result.json` 的原有消费者均退出 0，实际 CLI SHA256 `988ccd2a92f050808181e28d293c06659ad8ee89d0a04bf57df967590d3f2957`。覆盖文档/iframe/preload、本地 bytes 与撤销、同步 XHR 即时结果及原生 Fetch Continue/Fulfill/Fail、MCP；Worker 覆盖 importScripts 的顺序/错误/部分提交、重定向与 module、CSP、原子 transfer、Blob/File/MessagePort、终止失控脚本与挂起下载。此前 `document-delivery-20261020-211417/summary.json` 的 classic-fetch 和导航/stdin/HTTP/CDP 结果复用。网络权限/TLS 拒绝复用 1.1/2.1，不增加单独同步 TLS/DNS 矩阵。认证拦截仍明确不支持；同步 XHR 使用公开 CDPSession 的请求阶段 Fetch，而非自动开启认证拦截的 Playwright route。

## 5. XML 解析与流式解码

- [x] 5.1 合并 XML 文档和实例级流式解码，保持真实树与字节语义。

**根因 / 契约：** XML 输入被错误交给 HTML parser，分块输入丢失解码器状态。覆盖 RC-13/14；XML 优先复用 PR [#1206](https://github.com/h4ckf0r0day/obscura/pull/1206)，解码优先采用已有成熟编码库和可用补丁，不再自建编码框架。

**收口：** XML namespace/case/ownerDocument/parsererror 和序列化保持一致，不留 HTML 假包装；TextDecoder 按实例保存跨 chunk/fatal/BOM/flush 状态，页面和 Worker 复用同一路径。

**充分验证：** 一个命名空间 XML 修改/序列化/再解析消费者及非法 XML/外部实体拒绝；复用解码边界测试覆盖 split UTF-8、fatal、flush/reset 和独立实例。已有 WPT 中与改动相关的结果可复用，不以全套 WPT 认证作为新交付项。

**实跑证据：** 同一 Windows `render` CLI SHA256 `4f0267eb674dca4d5bd7d9b16184df1632bd9404b046068b3bf4aec9ef5472d0`。外部 `D:/temp/obscura-remediation/accepted-run/xml/evidence.json` 记录退出 0、33 个 XML 行为检查全部通过、外部实体请求为零；`decoder-cli.json` 记录真实 CLI 退出 0，复用现有 decoder 场景验证 split UTF-8、独立实例、BOM、fatal 后 reset、flush、ISO-2022-JP 跨块状态和 typed-array 各切分边界。永久 decoder/decoder-stream 回归仍纳入统一 focused nextest，不再为本项新增测试框架或全量 WPT 门槛。

## 6. 真实 WebSocket 连接生命周期

- [x] 6.1 合并真实 ws/wss 客户端，移除伪 OPEN 和丢弃 send 路径。

**根因 / 契约：** 页面状态与实际 socket 脱节。覆盖 RC-16，优先复用 PR [#1080](https://github.com/h4ckf0r0day/obscura/pull/1080)，适配本分支文档取消与所选客户端即可，不重写成熟帧协议。

**充分验证：** 复用 websocket_client 和现成 echo 消费者，观察文本/二进制往返、关闭、文档取消及拒绝；实际改动的 wss/proxy 后端用对应场景验证，不把 CDP 服务端 WebSocket 冒充页面客户端。依赖 1.1/4.1。

**实跑证据：** `accepted-run/websocket-closed/receipts.json` 记录实际 CLI SHA256 `9d2329fe1da97705d5f8d90dff10c902a3ff4eb41d1e574e1f8113d06ed3b8a0`；原消费者 `--clients both --routes http` 退出 0。普通与 stealth 的 ws/wss 文本/二进制往返、协议拒绝、CSP、导航/stopLoading/target-close 释放及其他页面存活均通过。此前 direct 成功结果复用；HTTP 代理夹具修正双向转发与 absolute-form 路由，未增加生产超时、重试或备用客户端。

## 7. 原生公钥 WebCrypto

- [x] 7.1 复用可用公钥实现与成熟后端，交付真实密钥和适用操作。

**根因 / 契约：** 缺少原生公钥能力，不能用 JS 假数据补接口。保留 RC-17 已批准的七类算法和适用 key usages/extractable/format/error 契约；先查对应 PR 的实际能力，只有缺失部分才补原生绑定。

**充分验证：** 复用已准备向量/消费者，覆盖各已实现算法的代表性真实操作、关键格式边界和拒绝；一次独立实现互操作与本地 DPoP 请求即可检验非同库自洽问题。无需为每个 CLI/CDP 入口重复所有算法组合；不引入磁盘密钥存储。

**实跑证据：** Windows `render,stealth` CLI SHA256 `c209347369516246eb0a1b30d504ec5e6268a605e8113469620c9f958b052f60`，worker SHA256 `9d2450f9e8a008181d8bba52c45879494a4426de7561985b8ba4787d332f6b57`。外部 `accepted-run/crypto-cli-finish-20261020/report.json` 状态 passed、CLI 退出 0，合并复用记录共 127 条成功消费者行：116 条 Chromium/Obscura 双向七类算法互操作、双方各四类向量/拒绝/包装/IndexedDB 场景、2 条本地 DPoP 和 1 条 CLI 向量。原 bare `--eval` 同步序列化 Promise 得到 `{}` 的失败保留于 `crypto-finish-20261020/`；只将启动器改用已有 `--dump text` 异步结算，不改算法操作或断言，也不重复已通过的 126 条检查。长度不符合 AES-KW 的四类 ECDSA 表示与 Chromium 同样不适用，不冒充成功操作。

## 8. 区域配置、权限与真实 RTC 路由

- [x] 8.1 合并区域配置传播与位置权限修复，保持 native Intl 和实际 RTC 路由一致。

**根因 / 契约：** 区域字段来源分散，位置配置被误当权限，RTC 地址声明脱离真实传输。保留 RC-18 已批准能力；优先复用 PR [#1228](https://github.com/h4ckf0r0day/obscura/pull/1228) 的可用部分，但权限仍由 origin/context 状态决定。

**收口：** 默认关闭不探测；显式配置优先、自动模式走有效路线、区域选择稳定；MMDB 查询和成熟 ICE/DataChannel 后端复用现有实现，不写概率认证或第二套协议。所需路由不可用明确失败，绝不伪造 candidate 或暗中直连。

**充分验证：** 现成本地配置/代理回显检查请求身份与 native Intl 一致、permission prompt/deny/grant/reset；一个真实双端 DataChannel 往返和有效路由的拒绝边界。针对实际不同的代理代码补例，不默认构造所有 TURN/TLS/OS 组合。依赖 1.1/3.1/4.1。

**实跑证据：** `accepted-run/regional-closure.json` 保留原生身份、Intl、位置权限 grant/reset/isolation、frame/Worker/popup 的实际观察；`regional-closure-mcp-corrected.json` 的真实 MCP stdio grant/position/reset 消费者退出 0，CLI SHA256 `370ef6bcc4cf7d43909b04ad960ae5f5c6b33be4125f97dff8280656a3646317`。复用 `regional-final.json` 的显式/自动/代理结果及 `rtc-direct/receipt.json`、`rtc-private-denied/receipt.json` 的双端 DataChannel 和拒绝证据。额外 Chromium 比较器未执行，不作为已通过结果或新增关卡。

## 9. 共享级联、布局、字体与绘制结果

- [x] 9.1 合并样式、布局、输入和矢量 PDF 修复，保持几何、绘制与资源选择同源。

**根因 / 契约：** 级联、baseline/inline owner、字体和 live surfaces 在测量/绘制/导出之间分叉。覆盖 RC-19/20/21/22/23/24/25。优先复用 PR [#1299](https://github.com/h4ckf0r0day/obscura/pull/1299)、[#1097](https://github.com/h4ckf0r0day/obscura/pull/1097)、[#1091](https://github.com/h4ckf0r0day/obscura/pull/1091)、[#1285](https://github.com/h4ckf0r0day/obscura/pull/1285)、[#1257](https://github.com/h4ckf0r0day/obscura/pull/1257)、[#1090](https://github.com/h4ckf0r0day/obscura/pull/1090)、[#1089](https://github.com/h4ckf0r0day/obscura/pull/1089)、[#1204](https://github.com/h4ckf0r0day/obscura/pull/1204)、[#1092](https://github.com/h4ckf0r0day/obscura/pull/1092)、[#814](https://github.com/h4ckf0r0day/obscura/pull/814)；其他具体布局/PDF 候选见映射，不因集中归属另建总括渲染框架。

**收口：** 共用正常 cascade、原生 layout/font registry/live surfaces；选中资源才请求；控件默认动作与实际命中/绘制一致。真实 PDF text/path/subset-font 替代整页 JPEG，迁移现有公开调用者，不保留 raster 兼容入口。未实现 touch/media 明确拒绝。

**充分验证：** 复用 PR 的 reduced fixture 和已准备的 selector/CSSOM/input/layout/Canvas/resource/PDF 消费者，按不同首处分歧选例，不复制同一路径。确认 geometry/paint/hit 一致、未选中的受保护 URL 零请求、可提取且可读 CJK PDF 与实际矢量对象。仓库要求的确定性与 broad top/bottom 配对在 11.1 统一执行一次；相同渲染证据供各 RC 复用。依赖 1.1 的资源传输。

**收口证据：** 原有 CSS/布局消费者于 `accepted-run/css-final-c0b2c2b7d861/evidence.json` 退出 0；保留该二进制实际 hash，不重跑未受影响的属性/几何路径。最后的 fragment 导航缺口位于 server interception 入口误走完整加载，不是截图缓存；统一回到既有同文档处理后，`accepted-run/render-final-8162eb904332/fragment-receipt.json` 退出 0（CLI SHA256 `8162eb90433267695b1ef9c580d9e297c1097e8fe269c135050440e608148f6c`）：同一 frame、无新 loaderId、DOM 删除保留、`:target` 更新，实际截图确认 Two 不再恢复。此前 Canvas、输入、资源和可提取 CJK 矢量 PDF 的对应已通过证据继续复用；不声称所有截图像素等同 Chromium。

## 10. 已有 profile 的提交、隔离与权限

- [x] 10.1 合并 cookie/localStorage 的可靠载入、提交和可见失败。

**根因 / 契约：** 现有 profile 的导航/干净退出提交不一致，写入失败未到达操作结果。覆盖 RC-26；优先复用 PR [#926](https://github.com/h4ckf0r0day/obscura/pull/926)，保留本分支已有事务和存储语义。

**充分验证：** 复用现成 profile 消费者，在同一 profile 的真实关闭重启后检查首个请求 cookie 与首个脚本 localStorage；另检验 origin/profile 隔离、私有权限和一个实际保存失败。CLI/MCP stdio/HTTP 仅覆盖其不同关闭代码，不重复存储矩阵。没有受控特殊卷就不新增挂载工程，也不声称支持未验证卷。不得新增持久 IndexedDB/CacheStorage 或 scrape profile。依赖 3.1/4.1。

**实跑证据：** Windows `render` CLI SHA256 `4f0267eb674dca4d5bd7d9b16184df1632bd9404b046068b3bf4aec9ef5472d0`，worker SHA256 `7c4086086121e68495baab0355ef6be2be530527ac4375635ace6836747b15f5`。`navigation-persistence-consumer.py --mode profile-boundaries --hung` 在全新 `D:/temp/obscura-remediation/accepted-run/profile-boundaries-fixed/` 退出 0；`acceptance-report.json` 记录首请求/首脚本、双 origin/profile、cookie 属性、私有 ACL 与已有宽 ACL 修复、删除及可见权限失败。独立 stdio/HTTP/CLI/serve/CDP 关闭重启和实际写入失败/恢复见同根目录 `navigation-profile/` 的 `stdio-read.json`、`http-read.json`、`serve-wire.json`、`cdp-wire.json` 和 `write-retry.json`。不声称验证了未提供的特殊卷或磁盘满场景；这些不是本项新增门槛。

## 11. 统一集成验证与交付

- [x] 11.1 统一修复集成错误、完成仓库必要验证并更新实际交付说明。

- 先解决已报告编译/运行错误；复用现有 target 和依赖，集中构建。受影响 release nextest 通过后，按仓库要求运行一次全量 `cargo nextest run --release --features render --no-fail-fast`。不使用 `cargo test` 或全树 `cargo fmt`。
- 执行精确 release CLI/worker build 和本地 obstacle course 33/33；所需 `render,stealth` 与 no-render 路径不互相冒充。CLI/worker 成对记录来源。失败后只重跑受到修复影响的场景，最终必要 gate 统一收口。
- 渲染按现有 deterministic 和 broad top/bottom 规则验证实际画面，必要时与 Chromium 同输入比较；不再新建截图/性能平台。只有声称性能改善或发现退化时才做等价分布对比，不把固定时长 heaptrack 设成普遍门槛。
- 复用 1.1–10.1 的实跑；仅补尚未覆盖且真实跨组的一条交互链。不同客户端都经同一入口时不重复证明；实际改了独立 adapter 才补对应入口。
- 更新受影响公开文档和变更记录，保留未解决调查的明确状态。核对 OpenSpec strict/apply、Issue/RC/PR 唯一归属和 todo 同步。不提交临时截图、日志或未完成消费者；不为完成任务数而改断言或宣称全部上游 Issue 关闭。

**最终交付：** 全量及各路径证据见 Current state 和 `accepted-run/delivery-final.json`；最终 render CLI/worker SHA256 为 `8162eb90433267695b1ef9c580d9e297c1097e8fe269c135050440e608148f6c` / `ad623ae7c82a7ac00ace969d8807331d6d1e4057ee5c55bd04246e16320e2841`，no-render 为 `80de30935c7970f884a80c1e8954b9cbd04c48dab7fd528d7d3c802a678c1a0b` / `7c1d99534dac5f005119deb83ea54646e0954493fd45948bb8c7b7661d5694d4`，render/stealth 为 `3e0d6c7d5f6080e8ecbe666a92d10cff25e2f7ac51866b4782895bb98ca03e01` / `120644ea3898cbd0bac8bfea5e07aa51611440eb4f55c7ffec62994ce342f4c5`。26 个 RC、102 个 Issue、37 个 PR 的主归属检查无重复；历史调查继续保留原始证据缺口，不将局部契约通过写成全部上游报告关闭。临时导航诊断脚本已删除，生成产物均在仓库外。
