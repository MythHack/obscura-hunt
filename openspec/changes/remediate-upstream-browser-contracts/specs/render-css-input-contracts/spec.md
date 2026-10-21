# Spec Delta

## Purpose

规定选择器、注册自定义属性、原生控件激活、布局几何、UA 默认值、计算样式、Canvas、SVG 和字体测量对真实页面及自动化客户端的共同契约。要求查询、计算、绘制、矩形与命中结果相互一致，完整保留各复合问题的剩余条款，不以新增方法名、空默认值或协议成功代替实际浏览器行为。

## ADDED Requirements

### Requirement: RC-19 选择器与 typed custom properties

浏览器 SHALL 让 `:nth-child(An+B of S)`、`:focus-within`、`:target` 在 DOM 查询、`matches()`、`closest()` 和样式匹配中表达一致的实时状态，并提供具有类型校验、继承与初始值语义的 `CSS.registerProperty()`。此要求承担 Issue 1295 除 TextDecoder 外的四项条款；解码由 RC-14 承担，不能据一项通过关闭另外四项。

#### Scenario: Nth-child 在筛选后的兄弟集合中计数

- **WHEN** 页面包含依次为 `.a`、`.b`、`.a` 的三个 li，使用 `li:nth-child(2 of .a)` 查询、元素匹配及 CSS 规则，并继续测试 `An+B`、选择器列表和 DOM 插入删除。
- **THEN** 基础夹具精确匹配第三个 li，后续结果按先筛选 S、再计算位置更新，查询与实际颜色一致；不匹配的兄弟不参与计数，DOM 查询、`matches()` 与 `closest()` 中的无效语法通过原生 parser 错误映射抛出 `SyntaxError` DOMException，而不是悄悄匹配空集。

#### Scenario: Focus-within 跟随真实焦点

- **WHEN** CDP 或页面把焦点移入、移出嵌套容器及可聚焦 shadow 子树，读取祖先的匹配结果并捕获规则产生的样式。
- **THEN** `:focus-within` 反映自身或相应焦点后代的真实焦点状态，失焦和移除后立即撤销；未包含焦点的兄弟不匹配，跨文档或封闭边界的访问不被 selector 支持绕过。

#### Scenario: Target 跟随文档片段而非静态标记

- **WHEN** 页面以 `#w` 导航到包含 `div#w` 的文档，随后更换 hash、清除片段以及执行历史前进后退。
- **THEN** `querySelectorAll(':target')`、`matches()` 与 `:target{color:red}` 的计算/绘制结果共同指向当前文档的真实目标；不存在的目标不匹配，不把旧文档、旧 hash 或仅获焦元素误认为 target。

#### Scenario: 注册属性参与真实计算和继承

- **WHEN** 页面注册 `{name:'--n', syntax:'<length>', inherits:false, initialValue:'3px'}`，读取初始值，再分别赋合法长度、非法颜色，并对 inherits:true 与 false 的父子夹具读取计算值。
- **THEN** 未指定时计算值为 `3px`，合法值按类型计算和序列化，非法值按注册属性规则回落，继承开关真正决定子元素值，依赖该值的宽度或颜色出现对应几何/像素变化；不是只把描述符保存在可调用的空函数中。

#### Scenario: 注册失败与文档隔离可观察

- **WHEN** 页面重复注册名称、使用非法名称或 syntax、缺少必需初始值，或者导航至新的独立文档再读取相同名称。
- **THEN** 重复名称抛出 `InvalidModificationError` DOMException，其他无效注册抛出相应 DOM/CSS 异常且不污染已有有效注册，独立文档不继承前一文档的注册；未注册属性保留普通 custom property 行为，不能用全局固定默认值伪造 typed 属性结果。

### Requirement: RC-20 原生激活与输入能力契约

浏览器 SHALL 让 first-summary 激活与水平原生 range 控件通过页面脚本、真实 CDP 指针/键盘入口产生符合浏览器取消、焦点、数值、绘制和事件语义的结果；尚未实现的 touch 和媒体模拟请求必须显式失败，已支持的媒体偏好必须真实作用于 JavaScript 与 CSS，不能以协议成功掩盖无效果操作。

#### Scenario: Summary 的脚本和指针激活均有效

- **WHEN** 页面调用 details 的第一个直接 summary 子元素的 `click()`，或自动化根据其实际矩形发送主键鼠标按下/释放。
- **THEN** details 的 `open` 属性与可见内容真实切换，按浏览器任务语义产生 `toggle` 事件；脚本点击保持脚本事件的信任属性，原生 CDP 输入产生真实受信任输入，二者都不靠直接替换用户页面逻辑实现。

#### Scenario: Summary 取消和 Toggle 合并正确

- **WHEN** click 处理器 `preventDefault()`，点击非首个 summary 或其中具有独立激活行为的交互后代，或在同一任务内反复改变 details 的 `open` 属性。
- **THEN** 被取消或不适用的激活不额外切换 details，交互后代保留自己的默认行为；程序修改与激活使用一致的 toggle 排队及合并语义，事件中的旧/新状态与最终 open 状态相符，不为每次中间状态同步伪造重复通知。

#### Scenario: Range 默认值与数值规则统一

- **WHEN** 页面创建默认 range 和带 min/max/step/value 的 range，并赋入越界值、非有限值、非法数值语法、步长不对齐值以及 `step=any` 的合法小数。
- **THEN** 默认 `.value` 为 `"50"` 且与 `.valueAsNumber` 一致，其他输入按 HTML range 的默认值、边界和步长规则清理；脚本赋值、属性变化、指针和键盘结果使用一致的合法值域，程序赋值本身不产生伪造的 input/change 事件。

#### Scenario: Range 由实际指针与键盘更新并绘制

- **WHEN** 本地页面显示宽 200px、min=0、max=100、step=10、value=50 的水平 range，CDP 在实际控制矩形约 75% 处完成主键点击，再向获得焦点的控制发送 ArrowRight，并执行一次按下、移动、释放拖动。
- **THEN** 与同字体及视口的浏览器对照夹具一致地得到点击后的 `80` 和按键后的 `90`，拖动沿实际轨道更新并按步长对齐；截图有可辨识的原生 track/thumb 且 thumb 随值移动；每次已完成的用户更新产生对应的受信任 input/change，拖动过程的 input 与最终 change 顺序正确。

#### Scenario: Range 的取消和边界不是特殊旁路

- **WHEN** 取消指针或键盘默认行为，操作 disabled 控件，使用次键、RTL、Home/End/方向键、拖出控件后释放或 `appearance:none`。
- **THEN** 取消、disabled 与不适用按钮不修改数值、不发出虚假的变更事件，键盘和 RTL 遵守浏览器方向及边界，拖动提交不因指针移出而丢失；`appearance:none` 不画原生主题但保留适用的控件语义，不把垂直滑块、作者 slider 伪元素或 touch 手势假称为已支持。

#### Scenario: Touch 在真实支持前显式不可用

- **WHEN** 客户端调用 `Input.dispatchTouchEvent` 的 start/move/end/cancel 生命周期，或请求尚未具备真实接触点及能力语义的 `Emulation.setTouchEmulationEnabled`。
- **THEN** 返回可供客户端判断的 unsupported 协议错误，不返回空成功，不制造 touch/pointer/click，不改变 `maxTouchPoints` 或 pointer/hover 媒体结果以伪装设备；普通鼠标和键盘输入继续可用。

#### Scenario: 已支持媒体偏好生效且未支持值不被吞掉

- **WHEN** CDP 设置、清除 prefers-color-scheme 与 prefers-reduced-motion，并导航两次，随后发送未实现的 media 类型、feature 名称或非法值。
- **THEN** 已支持值在 `matchMedia()`、CSS 计算结果与截图中一致生效，原生窗口设置传递同时保留 reduced-motion 与 dark-color-scheme，跨导航保留，清除恢复默认；未实现项返回明确错误且不部分提交同一请求中的其他项，不再静默忽略参数或重复声称已有两个偏好属于新功能。

### Requirement: RC-21 布局与盒几何保真

浏览器 SHALL 在 display 隐藏、行内基线与偏移、循环 flex 尺寸、浮动百分比宽度及 margin、表格内在尺寸和行内所有权的整个组合矩阵中保留正确的盒与片段语义，使真实绘制、DOM 矩形、visibility、offsetParent 和坐标命中一致；修复不得依赖站点名、固定例外尺寸或仅对单个已观察案例返回修补值。

#### Scenario: 隐藏表格行完全不生成盒

- **WHEN** 夹具同时包含 `display:none` 的 div、span、tr，以及置于隐藏祖先内的行；自动化查询矩形、`getClientRects()`、`checkVisibility()`、offsetParent 和 Playwright 可见性，并随后移除隐藏样式。
- **THEN** 隐藏状态无可见矩形，宽高为零，client rect 集合为空，visibility 为 false、offsetParent 为 null，不能被坐标点击或截图看见；恢复显示后按真实表格布局重新产生几何，不影响邻近可见行。

#### Scenario: 不同原子行内元素共享基线并留出下降空间

- **WHEN** 同一行含 10×10 与 40×40 图像、文字及不同高度 inline-block，分别测试默认/显式 baseline、top、middle、bottom。
- **THEN** baseline 元素在同一基线上排列，较小图像不是顶对齐，行盒为字体 descent 和必要 leading 留空间，top/middle/bottom 仍保留各自对齐语义；几何与截图一致，不能仅增加行高却不移动短元素。

#### Scenario: 行内相对垂直偏移影响真正的行盒

- **WHEN** 页面分别使用 sub、super、text-top、text-bottom、正负长度和百分比的 vertical-align，并结合不同字体尺寸与 line-height。
- **THEN** 文本或原子行内的实际位置、共享基线及行盒增长符合各值语义，百分比采用相应 line-height 基准；计算样式可读回相应值，零位移不增加虚假的偏移，table-cell 的对齐行为不被文本行内规则破坏。

#### Scenario: 循环 Flex 中的 Calc 最终使用正确基准

- **WHEN** 使用 Issue 767 的 1560px flex 祖先、自动宽度中间包装与 `width:calc(100% - 32px);padding-left:25px` 的 input，并比较无 flex 版本、普通非替换元素、替换图像与原生控件的变体。
- **THEN** 循环的非替换 preferred size 在内在贡献阶段按 auto 语义处理，最终 calc 根据最终 containing block 解析，input 不塌为零或由自己的 padding 决定基准；固定字体与边框条件下匹配浏览器对照几何，无 flex 的 1528px 计算结果及替换元素自然尺寸贡献不退化。

#### Scenario: 隐藏浮动不影响原子行内组合

- **WHEN** 浮动容器含文本、一个或两个 inline-block 或 img，组合加入无、普通隐藏、浮动隐藏兄弟，并改变隐藏节点的标签或文本长度。
- **THEN** `display:none` 兄弟无论是否有 float 都不影响父容器宽度、行分支、绘制或命中，删除该节点与隐藏该节点的结果一致；可见浮动仍占据真实浮动区，不能通过禁用所有 float 来通过隐藏矩阵。

#### Scenario: 浮动百分比宽度与两种 Margin 均正确

- **WHEN** 1600px 行容器内比较四项：float+83.33333333% 宽、再加 8.33333333% 左 margin、再改为 133px 左 margin，以及非 float 的百分比宽和百分比 margin；每行使用真实 clearfix 并跟随一个可命中的普通块。
- **THEN** 四列宽均约 1333.33px，百分比和像素 margin 分别采用正确基准定位，clearfix 行高非零，后续流内容排在浮动下方，原表单控件仍能命中；不得只修百分比 margin 而遗漏独立的 px-margin 情况，也不把正常非浮动布局改成 shrink-to-fit。

#### Scenario: 表格和 Inline-block 以可用宽度限制可换行内容

- **WHEN** 在 352px 普通容器与同宽 table 单元格中放入长的 auto inline-block 列表，矩阵包含 inline-block/inline li、带/不带逗号伪元素以及等价 div/span 内容，同时保留能放下的双词短控件对照。
- **THEN** 可换行长列表被可用空间约束并形成多行，表格 min-content 不被错误的整条 nowrap max-content 撑宽，指定宽度与邻近正文可用空间保持正确；能放下的短控件不被无故拆行，真实不可分割内容仍保留其正常溢出，不据此宣称未匹配状态的线上页面已经修复。

#### Scenario: 所有行内回退路径保留元素所有权和边缘

- **WHEN** label/span 等普通或嵌套 inline 与 input、button、img、textarea、inline-block、block、float、绝对定位兄弟混排，跨行或跨 block 分段，并带自己的水平 margin、padding、空内容或仅替换元素内容。
- **THEN** 所有原子回退与预展开分段产生正确的 owner 几何，margin/padding 反映在后续绘制位置；`getClientRects()` 按真实行片段合并，bounding rect 是正确片段并集，空 inline 和仅替换内容保留适用行高 strut；坐标点击命中真实 label 或 inline，不返回看似成功但不能交互的 0×0。

#### Scenario: 几何不得把不归属的后代并入

- **WHEN** 上述 inline 包含远处绝对定位后代、浮动后代、定位 inline、深层嵌套和被隐藏的后代。
- **THEN** out-of-flow/float 不错误拉长普通 inline 片段并集，定位 inline 继续提供正确的定位参照，隐藏内容不贡献盒；几何读取和捕获保持有界，不借放宽树循环、深度或执行期限保护获得结果。

### Requirement: RC-22 UA 样式与 CSSOM 值

浏览器 SHALL 为普通文本元素及 picture 提供可被作者正常覆盖的真实 UA 默认行为，并让 Issue 771 列出的全部 41 个计算属性及其列明的错误值在 CSSOM 中表达正确的初始、继承、级联或已计算结果；已建模的属性不得被固定默认值遮盖，绘制与几何不得为了伪造 CSSOM 数值而改变。

#### Scenario: 文本元素的完整 UA 默认矩阵可见

- **WHEN** 同一固定字体页面包含 small、big、sub、sup、code、kbd、samp、pre、blockquote、mark、u、ins、s、del、strike 和 q，并提供未标记 span 对照。
- **THEN** small/big 及 sub/sup 有正确相对字号，sub/sup 有实际下/上移，code/kbd/samp/pre 使用 monospace 且 pre 保留空白，blockquote 有默认缩进 margin，mark 高亮，u/ins 有 underline，s/del/strike 有 line-through，q 生成并测量嵌套引号；计算值、矩形与截图共同反映各项，不是仅加入声明但不画装饰。

#### Scenario: 相对字号和作者覆盖参与真实级联

- **WHEN** 作者在不同父字号下使用 smaller/larger，嵌套 small/big，并显式覆盖上述 UA 的 font、margin、background、decoration、quotes 或 display。
- **THEN** 相对字号以继承的父字号计算，嵌套产生相应变化，作者有效声明按优先级覆盖 UA；恢复或移除声明后重新得到正确默认值，不能用全页固定 13.33px/19.2px 模拟相对语义。

#### Scenario: Picture 与透明 Inline 保持有效图像布局

- **WHEN** 受控页面在固定宽度 containing block 内显示 `<a><picture><img style="width:100%;max-width:100%"></picture></a>`，分别使用无 background、全透明 background、可见 background，以及带伪元素、padding 的 anchor 变体。
- **THEN** picture 默认是 inline 透明包装，图片按真正 containing block 获得非零合理宽度并绘制；无 background 与全透明 background 不制造假的百分比基准，可见装饰和伪元素仍被保留，source 元素不生成多余盒；作者显式改变 display 时仍按 CSS 生效，不通过无条件抹除装饰或固定图片尺寸修复。

#### Scenario: 全部 41 项属性有语义正确的值

- **WHEN** 页面对普通 div 读取以下计算属性的初始值，再对其中可声明的项设置非默认声明、继承值或生效样式并重新读取：background-image、background-size、background-position、background-repeat、font-style、word-spacing、text-decoration、text-decoration-line、text-indent、list-style-position、border-spacing、caption-side、align-self、flex-grow、flex-shrink、flex-basis、grid-column、grid-row、transition-property、transition-duration、animation-name、animation-duration、animation-iteration-count、animation-timing-function、user-select、box-shadow、text-shadow、content、order、object-fit、resize、appearance、filter、mix-blend-mode、writing-mode、direction、unicode-bidi、isolation、contain、aspect-ratio、zoom。
- **THEN** 每项都有相应元素上下文中的有效 CSS 计算/解析值而非 `''`，默认值依各属性真实定义，非默认级联、继承、实际运行中的 animation-name 及动画 timing-function 等不被空值或固定 `none` 覆盖；属性访问与 `getPropertyValue()` 一致，不能把任意不支持属性都伪造为已实现的效果。

#### Scenario: Table 与最小尺寸不泄漏内部布局近似

- **WHEN** 读取 table、row/group/cell、list-item 和普通 div 的 display、flex-direction、align-items、min-height、transition-property，并加入真实 flex/grid item 及作者覆盖变体。
- **THEN** 保留真实 CSS box display，未指定的 table 不错误报告内部近似所用的 column/stretch；普通非 flex/grid item 的自动最小高度按解析值报告为 0px，而真实 flex/grid 自动最小尺寸遵守其上下文；默认 transition-property 为 all，作者设置的合法值和真实 flex/grid 行为不被默认修补覆盖。

#### Scenario: Outline 计算宽度与是否绘制分离

- **WHEN** 读取默认元素及 `outline-width:9px;outline-style:none` 的 outline-width，再把 outline-style 切为 solid。
- **THEN** CSSOM 返回绝对解析长度，默认 medium 按浏览器对照环境解析，9px 声明即使 style 为 none 也报告 9px；none 时无 outline 像素且不改变盒几何，solid 时按同一宽度绘制且仍不占据布局空间，不再把用于绘制的零 used width 当成计算宽度。

#### Scenario: 未连接元素不靠假默认掩盖语义

- **WHEN** 已读取的元素变为不属于任何文档的 detached 树，或查询不存在/未知的 CSS 属性，再重新连接到文档。
- **THEN** 保留相应 CSSOM 的空声明/未知属性语义，重连后重新反映真实级联；完整的 41 项修复不得把所有未知属性都填成非空字符串，也不让旧文档或旧样式快照泄漏到新位置。

### Requirement: RC-23 Canvas SVG 与字体度量

浏览器 SHALL 让 Canvas2D 的 HSL 色值真正影响像素，所有正常及回退捕获保留当前 Canvas 表面，SVGGeometryElement 返回真实路径/基本形状长度，并使原生按钮的测量与绘制采用一致的已选字体和 shaping 语义；不能以 stylesheet 色彩支持、原型链存在、空表面或仅换一个字体策略替代这些消费者结果。

#### Scenario: Canvas HSL 和 HSLA 产生正确像素

- **WHEN** 页面在真实 Canvas2D 上分别用逗号及空格 HSL/HSLA、带角度单位的 hue、合法十进制/指数/百分比 alpha 填色，并读取像素及截图。
- **THEN** `hsl(120 100% 50%)` 与对应逗号写法产生 `[0,255,0,255]`，alpha 经真实合成得到正确结果，stroke 等使用同类 paint 的消费者也一致，不因为 CSS.supports 或 stylesheet parser 接受该字符串就假定 Canvas 像素正确。

#### Scenario: 非 CSS 数值不能被当作合法 Alpha

- **WHEN** 页面在已有有效 fillStyle/strokeStyle 后设置使用 `0x`、`0b`、`0o` alpha、损坏函数或无效 CSS 数值的 HSL/HSLA，再绘制。
- **THEN** 无效赋值按 Canvas 颜色语义被忽略，保留之前有效样式，不解析成 JavaScript 数字，不回落为黑色或透明来伪装成功；合法边界的 clamp 与 hue 周期仍正确。

#### Scenario: 回退屏幕和 Print 捕获保留 Live Canvas

- **WHEN** 页面画入可辨识双色 Canvas，分别通过正常准备捕获、不同 viewport/base 的临时重建及 print-media 重建捕获；之后修改 Canvas 并再次捕获。
- **THEN** 各捕获在正确位置保留当次实时像素、alpha 和尺寸，后续绘制也出现在结果中，print 专用样式改变布局但不清空表面；不以重新播放脚本、复制一个旧截图或仅准备路径有像素作为回退路径验收。

#### Scenario: Canvas 表面不跨文档身份复用

- **WHEN** Canvas 被调整尺寸、移除或所属文档导航，新文档创建看似相同的元素，再走回退捕获。
- **THEN** 正常 resize 清空语义与新表面生效，已退休表面不泄漏到新元素或文档，不因节点标识复用显示旧像素，也不去掉既有像素/资源限制和取消保护。

#### Scenario: SVG 路径与基本形状提供真实长度

- **WHEN** 页面调用 path 的 `getTotalLength()`，覆盖绝对/相对命令、隐式重复、多子路径、闭合段、二次/三次 Bézier 及平滑命令、旋转椭圆弧和大小弧标志，并测试 line、polyline、polygon、rect/圆角 rect、circle、ellipse。
- **THEN** `M0 0 L3 4` 长度为 5，其他结果按真实局部几何与规定单位计算，可用于设置真实 stroke-dasharray/dashoffset；结果随几何属性和已支持的行内几何样式变更更新，不被 transform 或 pathLength 校准错误缩放，不以统一零值或外接框周长替代曲线/弧长。

#### Scenario: SVG 错误和受限几何诚实处理

- **WHEN** SVG 包含退化或无效几何、非法调用接收者、单位边界与 detached 元素，或者使用尚未纳入支持的 stylesheet-cascaded/animated 几何。
- **THEN** 退化/无效路径和错误接收者按适用 SVG/DOM 行为返回结果或异常，已支持属性/行内样式范围不会凭空扩大为完整动画几何支持；未支持的级联/动画能力明确披露，不把测得属性值伪称为该时刻已动画的实际几何，render/no-render 的同一几何调用保持一致。

#### Scenario: 按钮标签测量与实际绘制一致

- **WHEN** 在 390×844 固定字体夹具中显示 auto-sized `Save and sign in` 按钮及短标签，矩阵覆盖已选 system-ui/自定义 font-face/fallback、不同数值 font-weight、italic、letter/word spacing、文本变换、生成图标、padding 和 border。
- **THEN** 固有宽高容纳同一绘制字体和 shaping 后的标签与图标，未被作者约束时标签不因测量使用别的 weight/style 而意外折行或裁切；计算矩形、真实墨迹和点击区域一致，各边缘及变换只计算一次。

#### Scenario: 作者宽度和字体变化仍受真实约束

- **WHEN** 作者明确把按钮宽度限制到需要换行，或字体加载/样式切换导致真实字形度量改变。
- **THEN** 真实约束仍能产生相应换行/裁切规则，字体改变后测量和绘制一起更新，不能用固定最小按钮宽度、禁止所有换行或忽略 fallback/斜体来仅通过一个标签的截图。
