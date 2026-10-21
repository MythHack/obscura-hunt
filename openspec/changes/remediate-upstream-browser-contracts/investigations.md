# Investigation Register

本表保留 19 个历史报告及其不确定性，不再为每项建立独立实施或验收复选框。原始 Issue 不等于当前生产缺陷，局部等价输入通过也不等于原报告已修复。相关交付编号均使用收敛后的 [tasks](tasks.md)；它们表示已有根因边界，不是额外依赖或新增能力承诺。

## 证据与准入边界

- 原始报告保留在 [Issue 正文快照](evidence/open-issues.json)，历史分类保留在 [triage-report](evidence/triage-report.json)；不改写这些快照。
- 下列“保留观察”来自既有诊断运行的逐项观察及原始回执。本次只整理登记，不重新执行调查，也不把旧任务编号、建议命令或待测矩阵转成新的退出门槛；运行产物不是仓库内新增附件或公开下载链接。
- 既有诊断使用的旧二进制来自 `425b765baafa887edd812f79e91dd62908dd2eb6` 的 dirty 工作树，SHA-256 为 `e88dbfbd9ea00d25aa91ad186e57bf81995346ff4fa1165f3f837684cdca0d3f`，`render=true`、`stealth=false`。其诊断不能冒充 `2086082` 或本轮源码的验收；Chromium 回执只证明对应局部参考行为。既有记录中的源码修正也不等于新二进制已通过。
- 已有具体局部缺陷不能退回“材料未知”：INV-1151 的 classic fetch admission 和 INV-531 的重复 watchdog 归入 **4.1**；INV-866 的截止后 context 清除与同连接恢复失败由 **3.1** 负责 CDP 命令/context，**4.1** 配合原生任务/取消边界。它们沿原交付闭环，不再另建调查关闭任务。
- 其余未归因部分继续可见，但不因缺少原站材料、长时统计、特定平台或原报告的完整复现而阻塞本次交付。只有具体的当前局部复现，或本次改动触及的不安全路径证据，才按下列条件重新准入已有根因交付；若证据指向既有范围以外的能力，须另行决定范围。
- 原生不崩溃、不挂死、有效取消、真实浏览器语义和全部 26 个 RC 的已批准要求不变。共用构建、回归与交付只由 **11.1** 统一收口；本表不增加性能优化任务、长时基准任务或平台认证任务，也不以吞异常、增加等待、自动重试、重建页面或假接口关闭问题。

## INV-1283

- **原报告**：[Issue #1283](https://github.com/h4ckf0r0day/obscura/issues/1283)。0.2.5 在表单提交后反复记录 `cdApi is not defined`，后续 SDK 未插入；登录和交易读取仍成功，不是认证失败。
- **保留观察**：Chromium 本地等价链已经过实际 POST、延迟 classic loader、Worker/importScripts、跨源 SDK 插入与后续 context 调用；未重跑旧二进制已知 Worker intrinsic 失败，也未证明原 SDK 的首个分歧。
- **缺失材料**：提交后的 SDK/loader 字节、Worker/import 图以及相关 HAR/响应体；现有原始报告及评论没有这些材料。
- **相关根交付**：4.1 的 Worker realm 与文档资源生命周期；只有请求事实指向传输分歧时才关联 1.1。已批准的 Worker 修复不依赖拿到原站 SDK。
- **新准入条件**：当前本地链在 submit、accepted、loader、worker-message、sdk-executed、sdk-context 中出现可定位的首个缺失阶段，或改动路径存在相同不安全状态传播。不能仅由缺少 `cdApi` 推定一个新 API，也不开展真实账户/站点流程探测。

## INV-1242

- **原报告**：[Issue #1242](https://github.com/h4ckf0r0day/obscura/issues/1242)。脚本密集页加载后，简单 CDP 求值曾延迟约 1.7 秒；render/no-render、去脚本页面及 `awaitPromise` 两种模式表现不同，空闲调用也有额外延迟。
- **保留观察**：一次旧二进制诊断未形成完整回执；捕获轨迹最后为第二个 layout 页面后约 19813ms 发出的 `Runtime.evaluate("1+1", awaitPromise=true)`，id179 未见回复。没有完整 median/p95，也没有当前源码归因；未重跑该失败。
- **缺失材料**：原页脚本/task trace，以及能把未回复命令与渲染、同步脚本或 dispatcher 排队对应起来的当前证据。准备过的采样矩阵不是已完成测量。
- **相关根交付**：若为命令调度/拥有边界，归 3.1；若为已有布局改动导致的实际分歧，归 9.1。不新增通用 CDP 性能任务。
- **新准入条件**：相同输入与构建条件下的当前局部命令停滞，能定位首次阻塞；或本次改动在同一调度路径引入不安全的无界工作。历史延迟数字本身不构成当前回归或强制统计验收。

## INV-1222

- **原报告**：[Issue #1222](https://github.com/h4ckf0r0day/obscura/issues/1222)。Android 构建上 gzip module 记录为 0 字节，入口偶发不执行，后续动态 import 偶发请求失败，SPA 留在启动页；不能用手工重试 import 关闭。
- **保留观察**：包含 gzip classic、共享静态 import 和动态 lazy import 的最小图，在旧二进制和 Chromium 各三轮新 target 上通过；每轮四个预期脚本各请求一次，已检查执行标记和精确解码源码。该结果仅覆盖这个图。
- **缺失材料**：原 Android/SPA import 图及精确请求/响应字节；最小图通过没有解释原报告的间歇失败。
- **相关根交付**：1.1 的真实响应字节/网络事实，4.1 的模块与资源生命周期；复用既有消费者，不建立第二条模块加载路径。
- **新准入条件**：当前局部图首轮出现请求、解码体、body lookup、loading accounting 或执行标记的具体分歧，或改动触及的相同状态转换不安全。缺少原 Android 材料不转成平台认证要求。

## INV-1220

- **原报告**：[Issue #1220](https://github.com/h4ckf0r0day/obscura/issues/1220)。Windows render+stealth 诊断页面停留在 collecting/pending，另报 Canvas/WebGL 不工作、字体检查未结束；“所有站点检查变绿”不是引擎验收。
- **保留观察**：Chromium 的局部 Canvas 像素、字体 ready/测量、脚本与 frame 消费者通过；旧的已知 HSL 失败未重跑。CJK advance 为正不能证明字形可读，没有增加 WebGL/audio 或站点特判。
- **缺失材料**：原诊断页面/脚本快照与首个失败阶段；真实 WebGL 后端若被要求，是另一个须明确批准的能力边界。
- **相关根交付**：9.1 的真实 Canvas、字体及绘制结果，4.1 的脚本/frame readiness；既有 RC 验收不依赖第三方诊断站结论。
- **新准入条件**：当前本地消费者出现精确像素、字体加载、字形绘制或资源生命周期分歧，或本次变更破坏这些路径。不能由 pending 或 WebGL 不可用推定要添加伪构造器、扩大图形能力。

## INV-1155

- **原报告**：[Issue #1155](https://github.com/h4ckf0r0day/obscura/issues/1155)。旧 main 上创建/查询 20,000 节点约 850ms，对照 Chromium 约 150ms；报告把开销指向 bridge、提前分配 style/Proxy、observer 和重复插入扫描。
- **保留观察**：交接源码检查已有 scalar/insertion fast path，没有据此追加优化。组合诊断先在 INV-1242 停滞，未产出此项回执；V8 heap/阶段时间不能充当原生分配归因，没有可报告的当前性能收益或分配数字。
- **缺失材料**：能定位剩余 bridge/native 分配及存活栈的当前测量；旧 PR 收益不能代替当前证据。
- **相关根交付**：不新增性能交付。若证据落在已有文档/observer 副作用或几何消费者的错误，分别关联 4.1 或 9.1。
- **新准入条件**：当前已批准改动引入可归因的无效重复工作、无界分配或语义回归；没有这种证据时保留历史成本观察，不把 20k benchmark、原生 profiler 或跨平台测量变成退出门槛。

## INV-1151

- **原报告**：[Issue #1151](https://github.com/h4ckf0r0day/obscura/issues/1151)。历史报告提出普通传输缺 HTTP/2、CSS/script 下载分两波，以及 ready script 等待最慢无关脚本。
- **保留观察**：受控 CSS 350ms、classic 100ms、无关 async 900ms 输入中，旧二进制 CSS 于 3113.2ms 发起、3464.0ms 返回，classic 到 3467.1ms 才发起；交接源码检查定位 `Page::load_page` 在 `execute_scripts` 前等待 `fetch_stylesheets`。独立传输回执已证明五个资源在同一连接使用 HTTP/2/h2，不能继续把“没有 HTTP/2”当成当前缺陷。Chromium 原生 callback 参考已通过 CSS/classic 下载重叠及 parser/classic/module 顺序。
- **缺失材料**：该局部 fetch admission 缺陷不缺复现材料；交接中的 early-admission 修正交由 owning 交付整合，尚无新二进制通过结论。
- **相关根交付**：**4.1**，复用已有 classic fetch/readiness 屏障，在不改变合法 CSS 阻塞执行、parser/module 顺序的前提下提早接纳下载；不另建网络性能或 HTTP/2 任务。
- **新准入条件**：已确认的局部缺陷直接进入 4.1，不等待原站性能矩阵。额外原站慢点仅在当前请求时序证明新的首个分歧后准入；不能把合法的 CSS-blocked execution 误判为下载未接纳。

## INV-1103

- **原报告**：[Issue #1103](https://github.com/h4ckf0r0day/obscura/issues/1103)。兼容性提交和历史 Rust gates 已有结果，但当时 obstacle 为 32/33、Chromium 配对渲染未执行，也没有新的相对性能结论。
- **保留观察**：companion `2d7bc304fbb7ebefc44f770938192e3cede87dbd` 的修正 observer fixture 已包含 10px 卡片、1px sentinel 和逐批 re-observe；Chromium 原生回调达到 `io:50/cards50`。旧二进制仅 700ms 时 cards0 不是充分 callback 证据，未分类为引擎回归，也未重跑。
- **缺失材料**：原报告所指完整课程、配对渲染及成功 Playwright 工作负载的对应运行证据；单一 IO seam 不能替代整组结果。
- **相关根交付**：只复用 **11.1** 已有的集中回归/交付职责；若出现当前 observer/布局缺陷，再按实际边界关联 4.1/9.1。不再追加一套基准统计交付。
- **新准入条件**：已有固定夹具在当前构建出现具体语义失败，或本次改动破坏对应原生 callback/布局路径。历史未做完的相对性能比较继续标注未证明，不自动变成本轮强制退出门槛。

## INV-1102

- **原报告**：[Issue #1102](https://github.com/h4ckf0r0day/obscura/issues/1102)。除 html/body 外的多个元素返回固定占位几何，无法反映真实尺寸。
- **保留观察**：Chromium 对 137×41、223×73 两个块返回不同真实盒，对隐藏元素和 `display:contents` 返回空自身几何；旧的已知 hidden-TR 失败没有重跑。交接明确区分 no-render 能力边界，未声称其具有真实渲染几何。
- **缺失材料**：原报告对应的实际 build/features 与最小页面；准备过的当前 render/no-render 配对消费者不是已执行回执。
- **相关根交付**：9.1，已有 RC-21 负责真实盒/无盒几何，包括已确认的隐藏 table row 问题，不由本调查另建门槛。
- **新准入条件**：当前 render 局部输入仍返回占位盒，或改变的盒查询路径破坏已批准语义。不能用 no-render 合成几何推定 renderer 缺陷，也不增加独立 feature/platform 认证任务。

## INV-1050

- **原报告**：[Issue #1050](https://github.com/h4ckf0r0day/obscura/issues/1050)。嵌套 intrinsic/auto flex 与 open dialog 被报告出现 0px 高度或 3px 宽度；其中一个 dialog 的 computed width 为 800px。
- **保留观察**：带 800px 控制条件的局部拓扑在旧二进制和 Chromium 未出现所报塌缩，但不能推断原站修复。后来补充的独立 `/intrinsic-free` 路由没有固定 width/min-width 逃生条件，交接时尚未执行，不能引用为通过或不复现结论。
- **缺失材料**：原 modal 的 HTML/CSS/content 快照；原描述形状的无约束几何与实际绘制对照。
- **相关根交付**：9.1 的 intrinsic sizing/布局；既有布局要求不缩减，也不以一个通用正高度测试覆盖原问题。
- **新准入条件**：当前授权本地无约束输入出现可定位的尺寸传播分歧，或本次 sizing 改动存在相同不安全路径。控制条件下通过不关闭原报告，未执行的 free-input 也不是新增强制验收项。

## INV-993

- **原报告**：[Issue #993](https://github.com/h4ckf0r0day/obscura/issues/993)。browser UA 获得 JS shell 后不 hydration、networkidle 超时，CDP 客户端还报告 `No page for session`；换成非浏览器 UA 得到静态 HTML 不是浏览器修复。
- **保留观察**：Chromium 的本地 shell→preload→插入 module→延迟 API→三个 mounted result 链通过，另有实际 Playwright networkidle 和 advertised page WebSocket 无显式 session 的参考回执。旧的已知 preload 失败未重跑，没有切换 UA 规避。
- **缺失材料**：原 hydration scripts/API 请求图与响应体；Chromium 等价链通过没有归因原站问题。
- **相关根交付**：4.1 的资源 readiness 与 3.1 的目标/session 路由；若请求体/来源先分歧才关联 1.1。已批准 preload/路由修复照常交付。
- **新准入条件**：当前本地链中 shell、请求、preload 事件、module/API、target binding 出现首个可定位失败，或相关改动破坏拥有边界；不引入 plain-fetch 替代路径或站点 UA 特判。

## INV-925

- **原报告**：[Issue #925](https://github.com/h4ckf0r0day/obscura/issues/925)。历史页面在已有 insertAdjacentText/Element 修正后仍停留加载状态；报告没有确定后续所需能力。
- **保留观察**：[原评论](https://github.com/h4ckf0r0day/obscura/issues/925#issuecomment-5663677543) 给出了可独立处理的普通 DOM/真实 WebSocket 回复续行边界。Chromium 的本地 insertAdjacentText→timer→真实握手/frame/echo→onmessage→sessionStorage/location 导航通过；旧的已知假 socket 未重跑。
- **缺失材料**：原站特有停滞仍未归因；其挑战材料未收集，也不是验证普通本地消息续行的必要材料。
- **相关根交付**：6.1 的 RC-16 真实连接生命周期；若另有普通 timer/DOM/navigation 分歧才关联 4.1。真实 socket 缺陷已有 owner，不扩大到站点通行能力。
- **新准入条件**：当前本地握手/帧/事件与消息门控的 DOM/location 续行出现具体分歧。不得把挑战答案、OCR、指纹伪造或站点特判加入任务，原站加载状态也不是退出门槛。

## INV-888

- **原报告**：[Issue #888](https://github.com/h4ckf0r0day/obscura/issues/888)。macOS 12.7.6 x86_64 的日志先有 jQuery/ChildNode `parentNode` 异常，后有 deadline、watchdog、解码错误，最终 `Segmentation fault: 11`；JS 异常与原生崩溃必须分开。
- **保留观察**：旧二进制和 Chromium 的 12 项 before/after/replaceWith DOMString coercion 检查均通过；保留 jQuery 1.12.0 的实际资产及 domManip/param 路径，旧二进制 `param=a=&b=&c=42`、errorCount0。Windows 语义通过不是 macOS 原生崩溃的反证。
- **缺失材料**：匹配原构建的 stack/core/crash report，或实际会崩溃的隔离原生反例，以及携带崩溃的资产快照；现有进程日志不能定位首个 native fault。
- **相关根交付**：取得栈后才指定实际 owner；若证据落在已有文档/任务生命周期则归 4.1，不能预设 jQuery coercion 就是崩溃根因。
- **新准入条件**：具体当前原生崩溃反例或本次修改的 FFI/生命周期不安全路径立即进入实际 owning 交付；原报告保持未归因，不新增 macOS 认证，也不吞异常或放松 DOM 树约束。

## INV-878

- **原报告**：[Issue #878](https://github.com/h4ckf0r0day/obscura/issues/878)。下载流程报 shadow `HierarchyRequestError` 和后续 null 异常，标题称 crash；所附片段没有原生崩溃信号/栈，不能把 JS 层次异常改写为已证 native crash。
- **保留观察**：独立合法 shadow 插入/非法 host-including cycle 与精确 8 字节 attachment 消费者已保留。Windows 新 Chromium profile 即使显式禁用扩展仍被 IDM 外部拦截，把服务端 200/8bytes 改成 204；父代理的干净 Linux Chromium 在同一未弱化夹具通过 exact bytes/tree/recovery。它是干净参考，不是当前 Obscura 验收。
- **缺失材料**：原报告真正的 native crash 信号/栈，以及能排除下载外部拦截的原始输入/响应事实。
- **相关根交付**：1.1 的二进制响应/字节保真；只有真实 shadow 生命周期分歧才关联 4.1。合法树/循环保护必须保留，不增加站点挑战流程。
- **新准入条件**：当前本地 attachment 的源状态/字节与浏览器所收状态/字节有具体分歧，或 shadow 改动触及不安全树状态。外部 IDM 污染不能算引擎失败，现有层次异常也不能成为削弱保护的依据。

## INV-866

- **原报告**：[Issue #866](https://github.com/h4ckf0r0day/obscura/issues/866)。同文档 SPA 自导航后 context 被销毁，scroll/lazy load 后 `Runtime.callFunctionOn` 连续 30 秒超时，同连接不能恢复；另有 Maps 自身 TileError/ToPrimitive 日志。
- **保留观察**：本地 history retained handle=42、quiet timer 和 scroll fetch 已推进到未决 call。旧二进制约 10728ms 发出 id217，客户端 15s 先到期，约 40742ms 引擎才返回自身 30s 超时并清除 contexts；未达到配置的 10s 截止，也没有证明同连接/原 handle 恢复。最初 disposable cleanup 掩盖首错，最终 helper 已分离 cleanup，并允许捕获真实 engine deadline；没有把客户端等待变长当成修复。
- **缺失材料**：局部截止/context 失败已有具体回执；原 Maps TileError/ToPrimitive trace 和 reduced graph 仍缺，不能宣称整个原站问题已解决。
- **相关根交付**：**3.1** 拥有 CDP await/deadline、context/remote handle 与连接结果；**4.1** 拥有原生任务取消/推进。只使用同一 owning deadline，不增加另一套 watchdog，也不重建 session 掩盖状态丢失。
- **新准入条件**：上述已确认局部失败直接纳入已有联合交付，不等待原站材料。额外 Maps 错误只有当前独立反例或改动路径的不安全证据才再准入；完整站点稳定性不是新增门槛。

## INV-844

- **原报告**：[Issue #844](https://github.com/h4ckf0r0day/obscura/issues/844)。`getContext('webgl')` 返回 null；另报跨源 loader/脚本创建的 iframe 未出现，静态 iframe 可加载。两种现象不能自动合并成一个根因。
- **保留观察**：两个本地 origin、真实外部 classic loader 创建 child frame 的 load/message 链在旧二进制通过，Chromium 的修正 typed-wire-log 消费者也通过；最初异构日志 TypeError 是 disposable harness 问题。没有 fake WebGL，也没有原站完成结论。
- **缺失材料**：原 loader/CDN/frame 请求图与字节；真正 WebGL 后端是另行批准的能力，不是局部 frame 问题的默认修复。
- **相关根交付**：4.1 的 loader/frame 生命周期；无新增图形后端任务。
- **新准入条件**：当前本地跨源脚本请求、frame 创建、load 或 message 有首个具体分歧，或本次改动破坏相同边界。静态/动态局部链通过仅限该输入，不据此宣布原报告不复现/已修复，也不以挑战站点结果验收。

## INV-783

- **原报告**：[Issue #783](https://github.com/h4ckf0r0day/obscura/issues/783)。macOS 长期 CDP server 的 RSS 从约 33MB 跳到约 2.3GB，伴随空闲 50–100% CPU，重启会恢复；报告没有确定存活对象、queue 或 allocator 根因。
- **保留观察**：旧二进制 3220 cycles/317.7s 诊断中，RSS 初始 40.61MB、峰值 45.13MB、cycle3200 为 44.04MB、关闭 targets 后 39.19MB、关闭 connections 后 35.34MB、再 idle15s 为 30.49MB；private 从 21.37MB 到 cycle3200 的 25.67MB，idle 为 8.75MB；handles 142→128，idle 段累计 CPU 45.95s 未增加，V8 used 6962712→6962920bytes。七次 blocked-reader 尝试都未通过 advertised page endpoint 的精确 target-token 绑定，退出为 diagnostic-partial，不能声称已产生真实 queue pressure。
- **缺失材料**：原增长的当前局部复现及 retaining stacks；现有五分钟数据既不是原长时内存问题的反证，也没有证明总内存界限或完成 native allocation 归因。
- **相关根交付**：3.1 的 target/connection/queue 生命周期；若为页面或任务未释放则关联 4.1。不由此新增一小时/五小时 soak、原生 profile 或平台认证任务。
- **新准入条件**：当前局部生命周期产生明确存活对象/队列无界增长、关闭后仍可归因的 idle spin，或相关改动存在不安全拥有路径。必须先证明 target binding 和实际 pressure；RSS 单独变化、entry cap 或重启 workaround 都不能关闭或归因。

## INV-683

- **原报告**：[Issue #683](https://github.com/h4ckf0r0day/obscura/issues/683)。旧 MCP 导航在忙碌脚本页超过 75s 无返回、CPU 约 100%，生产曾持续 27 分钟；原生 watchdog 不能被关闭来规避。
- **保留观察**：加强后的本地 always-ready posted tasks、实际重复 host fetch 与远期合法 timer 三条路在旧二进制通过，有 readyCount/hostCount、server ping 与原生 callback 证据；同一 MCP 进程随后能求值 2 并完成 browser_close，远期 timer 未误触发。该结果不解释原站 livelock。
- **缺失材料**：原 ad-script/native-call trace 或实际导致无法返回的局部原生反例。
- **相关根交付**：4.1 的 RC-10/异步任务有界性与取消；没有独立“广告页兼容”或第二套 idle watchdog 交付。
- **新准入条件**：当前局部 ready queue/host call 在操作 deadline 后仍不返回，或本次调度改动存在绕过原生终止的不安全路径。历史未知部分不阻塞退出，但已有超时和恢复要求不降低，也不把等待远期合法 timer 当成 livelock。

## INV-531

- **原报告**：[Issue #531](https://github.com/h4ckf0r0day/obscura/issues/531)。旧 Astro/Vue/Vite 链有 3s module evaluation timeout、合法同步 hydration 被 watchdog 终止，并曾缺 `DOM.scrollIntoViewIfNeeded`/截图；报告建议的 no-watchdog/no-op 不是可接受修复。
- **保留观察**：合法带 padding classic 脚本在配置 12s phase 下于约 5012ms 被终止；交接源码定位到按 ≥10KB 大小启用的硬编码 5s cutoff，而两个 Page 调用点已有 owning phase watchdog。交接记载父代理已整合删除 `execute_script_guarded`、改用 phase 内 `execute_script` 的修正，保留显式 timeout API；已有 purpose-built Page completion/runaway-recovery 回归及原显式 timeout 测试，没有执行新构建/测试，不能记作验收通过。
- **缺失材料**：此重复 watchdog 缺陷不缺局部证据；原 Astro/Vue/HMR module 图仍缺，未宣称原应用已解决。旧滚动/截图报告不能由 watchdog 修正顺带宣布关闭。
- **相关根交付**：**4.1**，由现有 phase deadline 统一拥有所有 classic 大小的执行与取消，结合 INV-1151 的 moved calls 保留干净切换；不建立重复 watchdog、length proxy 或独立性能任务。现有滚动/绘制契约仍由 9.1 保持。
- **新准入条件**：已确认局部缺陷直接进入 4.1，合法慢脚本、失控终止和同 runtime 恢复沿其已有闭环收口；原 HMR/其他旧功能只有具体当前分歧或不安全改动证据才再准入，不以无 watchdog 模式、调大等待或假成功关闭。

## INV-527

- **原报告**：[Issue #527](https://github.com/h4ckf0r0day/obscura/issues/527)。正文实际报告 `%{articleInfo.title}%` 等模板变量未被脚本渲染；标题的 Canvas/WebGL/AudioContext 不提供这些能力缺失的证据。
- **保留观察**：gzip HTML→外部模板引擎→API JSON→可见 title/body 的本地链在旧二进制和 Chromium 通过，没有未解析 token，innerText 排除脚本源。最初局部 regex escaping 错误是 disposable fixture drift，已在取得语义结果前修正，不是引擎缺陷。
- **缺失材料**：原模板 HTML、inline/external scripts 与 API 响应字节；等价模板通过不解释原约 17KB 页面。
- **相关根交付**：出现实际脚本/module 生命周期分歧时归 4.1，响应解码/字节先分歧时归 1.1；不新增模板引擎或图形/音频能力。
- **新准入条件**：当前局部输入在 HTML 解码、引擎/API 请求、执行轨迹或可见 DOM 上出现首个分歧，或本次相关改动存在不安全状态转换。原材料未提供时继续登记未知，不作已修复或原报告不复现结论。
