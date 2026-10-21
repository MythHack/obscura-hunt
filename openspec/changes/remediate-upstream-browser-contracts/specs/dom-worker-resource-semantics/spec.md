# Spec Delta

## Purpose

规定页面可直接观察的 Document、子框架脚本导航、预加载、Worker、XML、文本解码以及本地资源与同步请求语义，使脚本和自动化消费者获得真实结果，而不是可替换的全局对象、HTML 包装、丢失的消息或提前返回的成功。所有行为均保留来源隔离、文档身份、取消、资源权限与执行期限边界。

## ADDED Requirements

### Requirement: RC-11 Document 与脚本资源生命周期

浏览器 SHALL 以不可任意替换的实时 Document 暴露每个 Window 的文档，在目标子框架中执行允许的 `javascript:` 导航，并让动态 `preload as=style` 的真实资源加载产生相应终态事件；上述行为不得绕过同源、sandbox、脚本执行或文档取消约束。

#### Scenario: Document 属性不可被页面替换

- **WHEN** 页面读取自身 `document` 的属性描述符，并分别执行非严格赋值、严格赋值及重定义尝试。
- **THEN** 描述符是 enumerable、non-configurable、有 getter 而无 setter 的访问器；非严格赋值不改变对象，严格赋值抛出 `TypeError`，不兼容的重定义失败；`document.body`、`cookie`、`scripts` 及文档身份继续可用，子框架拥有自身的实时 Document。

#### Scenario: JavaScript 子框架导航使用目标 Window

- **WHEN** 同源页面插入两个 iframe，一个导航到 `javascript:"<p id=x>hello</p>"`，另一个在 URL 脚本内调用 `parent` 回调并传入自己的 `window`。
- **THEN** 第一个子文档出现真实的 `p#x`，第二个回调收到该子框架的 Window；执行上下文、DOM 写入及对象身份均属于对应子框架，不修改父文档，也不向 HTTP 服务发送 `javascript:` 请求。

#### Scenario: 导航完成值和执行权限不是伪成功

- **WHEN** 子框架的 JavaScript URL 返回非字符串值，或因 sandbox、来源权限、取消或文档被替换而不允许执行。
- **THEN** 非字符串完成值不替换现有文档；被禁止的代码不执行，不获得父窗口私有数据，不安装替代文档；过期导航不能覆盖后来的子文档，顶层正常导航也不因错误的网络 scheme 分流而失败。

#### Scenario: 预加载事件与样式应用各有真实终态

- **WHEN** 页面动态加入成功、HTTP 失败及网络失败的 `link rel=preload as=style`，并等待其事件后把成功链接改为 stylesheet。
- **THEN** 每次有效加载在真实结果确定后触发一次对应的 `load` 或 `error`，等待该事件的页面逻辑能够继续；预加载阶段不提前应用 CSS，切换为 stylesheet 后计算样式与实际绘制反映该 CSS，原有普通 stylesheet 加载行为不退化。

#### Scenario: 预加载不得复活已退休文档

- **WHEN** 加载尚未结束时链接资源被替换或所属文档导航、关闭。
- **THEN** 旧结果不被派发给新链接资源或新文档，不重复触发终态，也不把旧 CSS 加入新文档；失败和取消不能被替换成无资源支撑的 `load`。

### Requirement: RC-12 Worker realm 与可转移消息

浏览器 SHALL 让 dedicated Worker 在独立 Worker 全局环境内执行，提供通过 `self` 可访问的内建对象、真正同步且按顺序执行的 classic `importScripts()`，以及双向结构化克隆和 MessagePort、ArrayBuffer 所有权转移；Worker 不得借用页面全局来伪装隔离，且必须保留脚本来源策略、网络权限、终止与执行期限。

#### Scenario: Worker 内建对象属于自身全局

- **WHEN** 页面启动一个真实 Blob Worker，Worker 返回 `self === globalThis`、`typeof Object`、`typeof self.Object`、`'Object' in self`、`typeof self.Uint8Array`，并修改自身内建原型和全局变量。
- **THEN** 自身全局相等且内建检查分别得到 `function`、`function`、`true`、`function`；修改只影响该 Worker，不污染页面或另一个 Worker；页面的 `window`、`document` 和私有变量在 Worker 中不可用，Worker 的 location 指向 Worker 脚本而非页面地址。

#### Scenario: ImportScripts 在返回前按参数顺序执行

- **WHEN** 本地受控页面启动 classic Worker，Worker 连续导入相对自身脚本地址的两个脚本，后者读取前者声明的全局，并在 `importScripts()` 下一条语句立即返回依赖计算结果。
- **THEN** 请求地址按 Worker 脚本环境解析，导入脚本在同一个 Worker 全局中按参数顺序执行，导入调用返回前依赖已经可用，结果由真实导入代码产生而非预置占位全局；允许的 data/blob 导入保留各自资源规则。

#### Scenario: 导入失败和终止不泄漏执行权

- **WHEN** 导入脚本发生网络、语法、MIME 或适用的来源/CSP 策略失败，或 Worker 在下载、排队、执行期间被终止。
- **THEN** 调用者观察相应的异常或 Worker 错误，未完成的后续导入不执行；已发生的前序脚本副作用按正常脚本语义保留；终止后不再产生页面消息、定时任务或迟到导入结果，也不绕过 private-network、file、TLS、代理与执行期限约束。

#### Scenario: Port 和二进制数据真实转移

- **WHEN** 页面把 MessageChannel 的一个端口与包含 `[0,255,128,42]` 的 ArrayBuffer 转移给 Worker，Worker 通过收到的 `MessageEvent.ports` 建立往返通信，并把处理后的缓冲区转移回来。
- **THEN** 发送端缓冲区在发送调用返回时已 detached，接收端获得正确字节和其环境中的对象，端口仍与未转移的对端真实连通，返回转移也有同样所有权变化；发送后修改未转移的普通对象不改变已克隆消息，任务及微任务的投递顺序符合浏览器语义。

#### Scenario: 无效转移不能部分成功或泄漏来源

- **WHEN** 页面或 Worker 发送重复端口、重复缓冲区、不可克隆值、已转移对象，或者尝试通过非授权 Worker 入口取得另一来源的资源。
- **THEN** 非法序列化产生相应 `DataCloneError` 而不交付半条消息、不部分夺走有效对象所有权；端口关闭后不再投递；Worker 消息的 `origin`、`source`、`ports` 符合该通信种类，不伪装成窗口来源检查已经通过，也不暴露其他文档或 Worker 的对象。

### Requirement: RC-13 XML 文档解析

浏览器 SHALL 将 `text/xml`、`application/xml`、`application/xhtml+xml` 和 `image/svg+xml` 解析成真正的 XML Document，保留大小写、根节点、命名空间、XML 节点与文档所有权，并支持这些节点的正常查询、修改和序列化；错误 XML 不得被 HTML 容错修复或包装成成功，解析不得执行脚本或展开外部 DTD/实体来访问网络及文件。

#### Scenario: XML 根节点和命名空间可供真实消费者读取

- **WHEN** 页面分别解析带默认命名空间的 `Edmx/DataServices`、无命名空间的 `root/child`、SVG 和 XHTML 输入，并读取 `documentElement`、`nodeName`、`localName`、`prefix`、`namespaceURI`、`contentType`。
- **THEN** 结果与原 XML 的真实根及命名空间一致，`Edmx` 和 `DataServices` 保持大小写，SVG 根不是 XHTML `html`；OData 风格的根与命名空间检查能够通过，不出现额外 HTML/head/body 包装。

#### Scenario: XML 节点可查询修改并保持归属

- **WHEN** XML 包含带前缀元素、同名不同 namespace 的属性、命名空间重绑定、注释、处理指令、CDATA 和字符引用，页面使用 namespace 查询和属性接口，创建、克隆、导入或采用节点，再序列化并重新解析。
- **THEN** 节点、文本值、限定名、属性 namespace 和作用域内的 namespace 查询保持正确，大小写敏感的查找不会错误命中；节点的 `ownerDocument`、父子关系及导入/采用后的归属正确，XML 序列化可恢复同等树语义，原活动 HTML 文档不受影响。

#### Scenario: 错误和不支持的 MIME 不被吞掉

- **WHEN** 输入存在不匹配的大小写标签、未绑定前缀、重复属性、多根、未闭合结构或非法字符引用，或者调用者提供 DOMParser 不接受的 MIME。
- **THEN** 错误 XML 返回可由真实 DOM 查询识别的 `parsererror` 文档而不是 HTML 修补后的树，不支持的 MIME 抛出 `TypeError`；以 `text/html` 解析的 HTML 仍执行惯常的 inert HTML 文档解析，不污染活动页面。

#### Scenario: 外部实体不能变成隐式资源访问

- **WHEN** XML 带有指向受控 HTTP 服务、private-network 地址或本地文件的外部 DTD/实体，或实体展开负载超出解析边界。
- **THEN** 解析不发起外部资源访问、不暴露外部字节且不无限展开；无法安全解析的声明以可观察的解析错误结束，不能靠静默丢失实体或套用 HTML 包装声称完整 XML 解析成功。

### Requirement: RC-14 流式文本解码

浏览器 SHALL 使每个 TextDecoder 按其编码、`fatal`、`ignoreBOM` 与 `stream` 选项保留独立的跨块解码状态，在结束时正确 flush 并为后续独立解码重置；TextDecoderStream 及真实读取字节的消费者必须获得同样正确的文本，不能用逐块宽松转换或错误替换来伪装完整字符。此要求承担 Issue 1295 的流式解码条款。

#### Scenario: 分块 UTF-8 输出完整字符

- **WHEN** 页面执行 `d.decode(Uint8Array.of(0xe2,0x82), {stream:true}) + '|' + d.decode(Uint8Array.of(0xac))`，并把四字节字符拆到不同 TypedArray 视图及空块中解码。
- **THEN** 第一项结果精确为 `|€`，四字节字符只在字节齐全后输出一次，不读取视图外的字节，不产生乱码或额外替换字符；与对完整字节序列一次性解码的结果一致。

#### Scenario: Flush、BOM 和实例隔离正确

- **WHEN** 两个解码器交错接收不完整序列，一个跨块接收 BOM，并分别使用默认选项和 `ignoreBOM:true`；随后以 `decode()` 或 `stream:false` 结束并开始新的输入。
- **THEN** 两个实例互不串状态，默认只在该次解码开头处理 BOM，`ignoreBOM:true` 保留 BOM 字符，流内中途出现的 BOM 不被错误删除；结束调用 flush 剩余序列，后续独立解码不继承旧尾字节或 BOM 状态，已支持的其他编码也保持其跨块状态。

#### Scenario: 非法和截断输入不被当成有效文本

- **WHEN** 页面解码非法 UTF-8、结尾截断的多字节字符或不支持的编码标签，分别使用 `fatal:true`、非 fatal 以及正常结束调用。
- **THEN** fatal 解码按规范抛出 `TypeError`，无效标签抛出 `RangeError`，非 fatal 仅按解码算法产生替换字符；未结束的不完整序列不会提前报错或丢弃，结束后不会把失败状态泄漏到新的独立解码。

#### Scenario: 流消费者获得真实解码结果

- **WHEN** 页面通过可读字节流和 TextDecoderStream 读取被拆块的 UTF-8 本地响应，直到流结束。
- **THEN** 消费者拼接出的字符串与完整字节解码一致，结束 flush 和 fatal 错误通过流完成或失败传播，而不是仅有 TextDecoder 方法名存在。

### Requirement: RC-15 本地 scheme 与同步 XHR

浏览器 SHALL 让允许的 `data:` 和 `blob:` 资源消费者读取准确的字节与 MIME、遵守对象 URL 的来源及撤销语义，并使 `XMLHttpRequest.open(..., false)` 的 `send()` 在真实请求完成或失败后才返回；本地解析与同步执行不得绕过原有 transport、拦截、代理、凭据、重定向、CORS、file、private-network、TLS、取消及执行期限边界。

#### Scenario: 本地脚本和模块实际执行

- **WHEN** 同源受控页面动态导入 literal、base64、percent-encoded 的 JavaScript data 模块及 Blob 模块，并分别加载 classic Blob 脚本、module data/blob 脚本和已有 classic data 脚本。
- **THEN** 模块真实导出 `42` 或 `7` 等夹具值，脚本产生规定的全局副作用并触发相应加载事件；本地 URL 不被当作 HTTP 请求，模块图的解析、身份与依赖错误仍遵守其 URL 和脚本类型语义。

#### Scenario: Fetch 保留二进制和 MIME

- **WHEN** 页面对 percent/base64 data URL 和包含 `[0,255,128,65]`、指定 MIME 的 Blob 对象 URL 调用 fetch，并读取 arrayBuffer、文本与 Content-Type。
- **THEN** 返回字节逐字节正确，MIME、charset 与文本解码来自资源本身，不因预先转成 UTF-8 文本而损坏二进制；同一 Blob 的脚本、模块、Worker 与 fetch 消费者不读取彼此不一致的内容。

#### Scenario: 撤销和来源限制不被本地分支绕过

- **WHEN** 对象 URL 被撤销后开始新的 fetch/脚本/模块/Worker 读取，或另一不被允许的来源尝试消费该 URL，或资源不是合法 JavaScript MIME、data 编码损坏、模块的相对依赖无可用基址。
- **THEN** 对应消费者以规定的网络、加载或模块错误失败，不从残留文本副本复活资源、不执行错误 MIME 代码；已经成功取得资源的操作遵守浏览器的撤销时点语义，不被错误追溯删除；本地 scheme 的支持不开放任意 file 或 private-network 访问。

#### Scenario: 同步 XHR 的下一条语句看到完整结果

- **WHEN** 受控服务分别返回带自定义响应头的 200 配置、404 正文以及二进制响应，页面以 `open('GET', url, false)` 调用 `send()` 并在下一条语句读取结果。
- **THEN** 请求已真正到达服务，`readyState` 为 DONE，status 分别为真实的 200 或 404，允许暴露的响应头、responseURL 和相应 response/responseText 已完整可读；不必等待 Promise、计时器或另一轮页面求值，异步 XHR 模式仍保持异步。

#### Scenario: 同步请求不能换一条不受管控的通道

- **WHEN** 同步 XHR 经过受控代理和请求拦截发生重写、履约、失败、重定向或凭据处理，或触发 CORS、file、private-network、无效 TLS、取消及执行期限限制。
- **THEN** 可观察的服务/代理记录、响应和错误与适用的浏览器网络策略一致，重定向及改写后的地址仍受校验；网络失败按同步 XHR 语义在返回前体现为失败，不返回伪造的 HTTP 状态、空正文成功，也不发生不受约束的阻塞或原生执行上下文重入。
