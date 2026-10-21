# Spec Delta

## Purpose

本能力规定控制面 HTTP 路由、浏览器级目标发现、页面端点绑定、会话订阅、网络事件与实时 DOM 发布的外部一致性。自动化消费者必须能对真实存在且可操作的目标取得正确事件和响应体，同时不能借全局目标可见性共享另一上下文的 Cookie 或身份，也不能破坏原生执行线程、文档代次与节点所有权边界。

## ADDED Requirements

### Requirement: RC-06 控制面路由与敏感产物边界

控制面 SHALL 仅依据合法 HTTP 请求行中的实际请求目标和受支持方法选择发现端点或 WebSocket 端点，保留既有绑定、Host、Origin 和认证约束，不得依据请求头值中的子串路由。公开源码与交付产物 SHALL 不再携带已经提交的本机构建转录，并防止同类生成日志再次成为公开交付内容。Cookie 私有权限与持久化错误的可见性由 RC-26 共同约束，本项路由与产物验收不代表该 Cookie 子项已完成。

#### Scenario: 精确发现端点保持可用

- **WHEN** 已通过控制面访问检查的客户端使用支持的方法访问 `/json`、`/json/list`、`/json/version` 或 `/json/protocol`
- **THEN** 只返回与该精确目标对应的发现响应，所公布的页面和浏览器 WebSocket 信息与真实可操作目标一致，不依赖其他请求头恰好包含某个端点名称

#### Scenario: 头值不会劫持 WebSocket 路由

- **WHEN** 客户端对有效 `/devtools/page/{targetId}` 发起 WebSocket upgrade，并在普通请求头值中放入 `/json/list`、`/json/version` 或 `/json/protocol`
- **THEN** 请求仍按实际页面目标执行合法 upgrade，不返回被头值选中的 JSON；升级后真实 `Runtime.evaluate` 能操作该目标

#### Scenario: 近似路径与畸形请求不冒充支持端点

- **WHEN** 客户端请求 `/json/versionify`、额外路径段、包含发现端点字样的其他目标，或发送无法合法解析的请求行及不支持的方法
- **THEN** 控制面明确拒绝或按该目标的既有非发现语义处理，不返回相似端点的成功响应，不绕过访问控制，也不把畸形输入静默解释为有效页面连接

#### Scenario: 构建转录不是公开交付物

- **WHEN** 操作者查看此变更之后生成的公共源码归档或发布交付清单，并在后续构建产生新的本地日志
- **THEN** 已提交的本机构建转录不在公共归档或发布清单内，新生成的同类日志不被默认纳入公开内容；不以仅隐藏日志展示代替移除公开内容

### Requirement: RC-07 网络事件与响应来源

CDP SHALL 在真实请求发生时及时发布与实际发送关联的 Network 请求事件，保留请求头、请求体存在性及可表示的 postData，并以 `Network.requestWillBeSentExtraInfo` 报告实际发送的请求头。响应、重定向、最终文档 URL、最终方法、请求体和 frame/loader 关联 SHALL 来自对应真实请求，不得借响应头冒充请求头、固定填 GET、等待无关 timer，或让观察会话夺走拥有客户端的导航响应。

#### Scenario: POST 的真实请求信息到达自动化客户端

- **WHEN** 已启用 Network 的客户端使页面发送带 `X-Probe`、Content-Type 和 `hello=world` 的 POST，并在普通及 stealth 模式读取事件
- **THEN** `Network.requestWillBeSent` 包含实际请求方法、请求头、`hasPostData: true` 和对应 postData，`Network.requestWillBeSentExtraInfo` 具有实际发送的最终请求头；本地服务端记录与客户端观察一致，而不是响应字段的复制

#### Scenario: 没有无关唤醒仍及时发送事件

- **WHEN** 页面发起受控异步 fetch 或 XHR 后空闲，不再执行 evaluate，也不设置额外 timer
- **THEN** 客户端在请求和响应各自发生后及时取得对应 Network 事件，正确关联 requestId、frame 和当前 loader，不必等另一个命令或无关任务唤醒页面

#### Scenario: 表单最终响应与实际重定向方法一致

- **WHEN** 真实原生 POST 表单分别得到直接响应、303、307、308 或多跳重定向到结果文档
- **THEN** Playwright 的导航等待取得已加载结果文档的真实 Response，CDP 最终 URL 与页面一致，直接 POST、307 和 308 后的最终方法及请求体仍为实际 POST，303 按浏览器规则报告转换后的 GET；服务器没有重复收到原提交

#### Scenario: 多会话不能随机夺走导航事件

- **WHEN** 页面客户端和附加观察会话同时启用相应域，再由页面点击或表单提交触发导航
- **THEN** 页面客户端稳定收到与自己的导航关联的请求、响应和生命周期结果，观察会话按 RC-08 的订阅规则收到自身应得副本；结果不取决于会话枚举顺序，不以只给某一个随机会话发送事件替代正确路由

#### Scenario: 拒绝与二进制请求不伪造成功证据

- **WHEN** 请求在授权、拦截或网络阶段失败，或者包含不能作为文本表示的二进制请求体
- **THEN** 客户端取得与真实结果相符的失败或二进制请求信息，不报告未发送的请求头为已发送，不伪造成功响应或 lossy postData；请求体与响应体仍遵守 RC-03 的字节保真和既有保留上限

### Requirement: RC-08 浏览器目标与会话所有权

CDP SHALL 在同一浏览器实例的 HTTP 发现和所有获准浏览器连接之间公开同一组真实目标，并使附件会话和页面 WebSocket 对该目标执行真实操作。目标可见性 SHALL 不改变其浏览器上下文、原生执行线程和页面身份，也不共享独立上下文的 Cookie、请求头或 UA。Page、生命周期、Runtime 和 Network 事件及响应体的可用性 SHALL 由目标与每个会话的订阅共同决定，单个会话的禁用或脱离不得清空仍有订阅者使用的数据。

#### Scenario: 新浏览器连接发现同一真实初始页

- **WHEN** 新启动的服务尚未收到 createTarget，两个浏览器 WebSocket 分别执行 `Target.getTargets` 和 `Target.setDiscoverTargets`，客户端同时读取 HTTP 页面发现列表
- **THEN** 三者观察到同一个真实初始 about:blank 页面标识，发现订阅获得已有目标的 `Target.targetCreated`，客户端能够附着并执行页面命令；不得只公布无法操作的固定目标条目，也不得因多开连接制造重复初始页

#### Scenario: 跨连接附件操作到目标原所属执行环境

- **WHEN** 连接 A 创建页面并保存页面变量，连接 B 发现并附着同一目标，再执行读取、修改、导航及关闭
- **THEN** B 操作的就是 A 所见的真实页面，变量与导航结果相互一致，关闭结果对所有连接可见；原生页面和执行环境仍在其拥有线程执行，不搬移隔离环境、不用全局锁抹掉既有跨连接并行能力，也不只转发元数据而拒绝实际命令

#### Scenario: 页面端点隐式绑定其公布目标

- **WHEN** 客户端连接发现响应中的 `/devtools/page/{targetId}`，不提供 sessionId 发送 Runtime.enable、Page.navigate 和 Runtime.evaluate，并同时保留浏览器端点与显式 flattened 会话
- **THEN** 不带 sessionId 的页面命令操作该页面且得到真实求值结果，浏览器范围命令和显式会话仍按自身作用域路由；不存在或已关闭的目标明确失败，不静默创建一个替代页

#### Scenario: 目标共享不等于上下文数据共享

- **WHEN** 两个独立浏览器上下文分别设置不同 Cookie、UA 和额外请求头，一个连接发现另一上下文的目标并附着操作
- **THEN** 全局目标与 browserContextId 可见，但每个页面发出的请求仍使用其所属上下文的状态；stealth 原生发送在补齐默认值前规范化额外头名称，显式 `User-Agent` 与 `Accept-Language` 大小写不影响优先级且各只发送一次；新连接默认上下文不继承另一上下文的私有数据，附着不改变目标所属上下文

#### Scenario: 域事件仅发送给对应订阅会话

- **WHEN** 同一目标有多个会话分别启用或禁用 Page、生命周期、Runtime 和 Network，另一个目标也有已启用会话，并触发导航、同文档变更、子 frame 变化及脚本请求
- **THEN** 每个事件仅到达同一目标中已启用相应域的会话，各副本保留正确关联；未启用或属于其他目标的会话不收到事件，导航在途期间仍保留已公告子 frame 状态，不重复公告或丢失后续销毁事件

#### Scenario: 响应体保留遵守最后订阅者边界

- **WHEN** 会话 A 和 B 均启用 Network 并观察实际响应，A 执行 Network.disable 或 detach，B 随后执行 Network.getResponseBody，最后 B 也离开或目标关闭
- **THEN** B 在既有容量与淘汰边界内仍可取得准确响应体，A 的操作只撤销自己的订阅和可用视图；最后订阅者离开或目标关闭后相关保留数据和订阅被清理，不跨目标泄漏，不无限增长

### Requirement: RC-09 实时 DOM 发布

CDP SHALL 在客户端已经取得文档或已发布子树后，将该已发布范围内的真实插入及时作为 `DOM.childNodeInserted` 发布，使客户端无需重新获取整棵文档就能追踪新节点。新发布节点 SHALL 使用与现有 DOM 查询、对象解析和命中测试相同的页面、frame、文档代次作用域身份；不得用原始节点序号或页面脚本可伪造的对象表替代。现有四个节点查询方法保持回归约束，本项不重做它们，也不把明确不支持的 shadow/frame piercing 宣称为完整支持。

#### Scenario: 文档发布后的插入可由真实客户端继续操作

- **WHEN** 原始 CDP 或 chromedp 风格客户端先执行 DOM.getDocument，然后页面脚本向已发布父节点追加或在已有兄弟之前插入按钮
- **THEN** 客户端及时收到该会话的 `DOM.childNodeInserted`，其中 parentNodeId、previousNodeId、节点内容和身份反映真实树顺序，随后可用该 nodeId 查询属性、解析对象并操作按钮，无需再执行 DOM.getDocument

#### Scenario: 未发布分支按客户端已知范围公开

- **WHEN** 客户端只取得浅层文档，页面在尚未展开的分支插入节点，随后客户端请求该分支的子节点
- **THEN** 发布结果与真实当前子树一致，既不引用客户端从未收到的父节点，也不在后续展开时重复插入同一前端节点；当前已发布范围以外的变化不能伪装成完整深树已同步

#### Scenario: 多会话与隔离世界保持同一文档身份

- **WHEN** 同一页面的两个会话取得不同深度的文档，页面主世界或原生隔离世界插入同一个真实节点，随后分别查询该节点
- **THEN** 各会话仅收到符合自身已发布范围的事件，插入事件与 querySelector、requestNode、describeNode、getAttributes 的节点身份一致；对普通树的 requestChildNodes 和有效几何条件下的 getNodeForLocation 保持既有行为，不重新引入四个方法缺失

#### Scenario: 旧文档和伪造句柄不能污染新树

- **WHEN** 页面导航、frame 替换、会话禁用 DOM 或脱离后，旧文档有延迟插入，或客户端提交其他页面、其他 frame、旧代次和伪造 JS 对象对应的标识
- **THEN** 不向新文档或无关会话发布旧插入，失效或外来标识按现有所有权规则被拒绝，新文档不复用可混淆的旧身份；正常文档和 frame 代次仍与 RC-08 路由一致

#### Scenario: 未实现的穿透能力仍诚实拒绝

- **WHEN** 客户端要求穿透尚不支持发布的 shadow 或 frame 子树
- **THEN** 请求继续明确报告该穿透边界不支持，不返回省略子树的成功结果；普通 light-DOM 的实时插入成功不作为完整 shadow/frame 支持或整个上游 issue 全范围完成的证据
