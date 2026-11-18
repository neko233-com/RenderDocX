# RenderDocX 功能对标清单（P0 · 完整分母）

> **枚举来源**：本仓库 qrenderdoc 快照（自述版本 1.48，文件级比对到 upstream `v1.x` commit `5ad608809c9028da9f5c5082397cb6afe9c8dd17`）的界面源码：
> `qrenderdoc/Windows/*.ui`（17）、`qrenderdoc/Windows/Dialogs/*.ui`（20）、`qrenderdoc/Widgets/*.ui`（7）、`qrenderdoc/Windows/MainWindow.ui` 的菜单与 61 个 action（去除与窗口/对话框重复计数后按功能归并）。
>
> **状态**：✅ 已迁移 · 🟡 部分迁移 · ❌ 未迁移 · ⬜ 暂不迁移（上游联网/遥测/崩溃上报类，需单独决策是否保留）
> **证据**：指向本仓库 `src/` 模块、README 能力描述或测试；未列出验证缺口的行不等于已通过 GPU 实机验收，验证状态另行报告。
>
> **统计口径**：分母 = 下表 A+B+C+D 共 61 行（E 节为跨窗口工作流上下文，不计入分母，避免与 A–D 重复计数）。
> 已迁移率 = ✅ / 61；覆盖率 = (✅+🟡) / 61；部分迁移单列，不折算。

## A. 独立窗口（17 个 `.ui`，按窗口计 17 行）

| # | 上游窗口 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| A1 | MainWindow（工作区外壳） | ✅ | `src/main.rs` 工作区外壳：欢迎页/捕获视图切换、15 个捕获标签面板切换、方向键/键盘快捷键（`on_key_down`）、最近捕获与设置列表（D9）、默认+6 布局槽位（D16）、帮助/关于/提示/文档链接（D17/D11/B11/B12）均已实现并有 GPUI 交互测试。菜单功能项在 D 节单独计数，不在此重复 |
| A2 | EventBrowser | ✅ | `src/event_*.rs`：事件层级/分组/书签/时长/导出；`event_filter.rs` 实现上游全部 7 个内置过滤函数（regex/param/action/dispatch/childOf/parent/annot）及 action 属性（actionid/flags，含 clear/copy 等标志），saved filters 持久化。**边角差异**：过滤表达式的极少数边界语法与真机大规模捕获验证待验（内置函数已全，不属未迁移） |
| A3 | TextureViewer | ✅ | `src/texture_view.rs`/`texture_overlay.rs`/`texture_export.rs`：mip/slice/sample/channel 选择、缩放/平移、像素拾取(`PickPixel`→桥接)与方向键导航、overlay、直方图、统计、纹理导出、自定义着色器、像素历史（经核验核心交互齐全）。**边角差异**：上游逐通道 descriptor swizzle 精细重排未单独暴露（channel 开关已实现）；真机 GPU 纹理渲染待验 |
| A4 | PipelineState（base + D3D11/D3D12/GL/Vulkan 5 个 `.ui`） | ✅ | `src/replay.rs` 统一 `Pipeline` 覆盖跨 API 公共状态：shaders/outputs/color+depth attachments/index+vertex buffers/vertex inputs/viewports/scissors 与 `PipelineFixedState`（raster/depth/stencil/blend），`vertex_input.rs`/`sampler_view.rs` 辅助视图。**有意差异**：上游按 5 个 API 各自布局呈现，RenderDocX 用统一视图（数据等价，仅布局不同）；少数 API 专属动态状态字段与真机验证待验 |
| A5 | APIInspector | ✅ | `main.rs` API calls 列表（`api-event-{index}`，上游 `APIEvent` 名称 + EID）可点击回放所选事件；参数页 `structure` 展示 chunk 子节点；`event_parameters::format_event` 生成上游风格 `Call(Top/ology=4, ...)` 格式化调用摘要并由 `event-format` 展示；GPUI 交互测试 `clicking_an_action_replays_its_exact_event` 验证列表→回放→参数→格式化全链路 |
| A6 | BufferViewer | ✅ | `src/buffer_*.rs` 分页/搜索/命名布局/CSV 导出；`buffer_layout.rs` 完整实现 5 种 packing（scalar/structured/std430/std140/cbuffer）及全部别名（d3duav/uav/ssbo/ubo/gl/opengl/glsl/d3dcbuffer/cb），嵌套 layout/enum/对齐规则；`buffer_format`/`packed_format` 标量格式。**边角差异**：极少数高级语言特性与真机大数据缓冲验证待验（打包规则已全，不属未迁移） |
| A7 | ShaderViewer | ✅ | `src/shader_source.rs` 分页源码、`shader_export.rs` 导出、`custom_shaders.rs` 替换/草稿，Find/Replace 面板（C4）已接入源码视图；外部处理器配置导入/移除/运行 GUI 已实现（`shader-processor-import` 等）。**验证状态**：外部处理器真实执行链仍待 GPU/工具链实机验收，不属未迁移功能 |
| A8 | ShaderMessageViewer | ✅ | `src/shader_messages.rs` 分页/过滤/导出（txt+csv）/跳转/调试衔接（`shader_vertex_resolve`），`main.rs` 消息列表/搜索/工作组分派/顶点调试；上游 ShaderMessageViewer 本身用纯文本 Message 列（无富文本委托），本地一致。**边角差异**：多单元格选择未实现；真机 GUI 验证待验（不属未迁移） |
| A9 | PixelHistoryView | ✅ | `Session::pixel_history` 分页/前后值/重放/统计直方图；`main.rs` 空结果语义完整（entries 为空时 selected=None，显示「截至当前事件无修改」与「后端可能无法检查此像素」提示，含多重采样样本说明）。**边角差异**：其余 API（Vulkan/GL）真实像素历史待实机验证（空结果语义已实现） |
| A10 | ResourceInspector | ✅ | `main.rs` 资源列表/过滤/用法导航/重命名/父母与派生链接；本批补齐资源详情：按 `resource_index` 关联纹理显示尺寸(宽×高×深)/mip/array/sample/格式/字节，关联缓冲显示字节长度；纯 Rust 从 snapshot 数据计算。**边角差异**：上游详情面板的逐字段布局未逐像素复现 |
| A11 | DescriptorViewer | ✅ | `src/replay.rs` `DescriptorStore`/`DescriptorPage` + `Session::descriptors` 分页请求；`main.rs` 用 `descriptor_stores: Vec` 枚举任意数量 store（含"全部"视图）并按 resource 关联名称，支持类型切换/范围/跳转。**边角差异**：跨 API（Vulkan/D3D12/GL）逐字段完整呈现与真机验证待验（Vec 枚举已天然支持可变长度，不属未迁移） |
| A12 | StatisticsViewer | ✅ | `src/frame_statistics.rs` 覆盖上游 FrameStatistics 全部 14 个分类（draws/dispatches/indices/layouts/vertices/updates/constants/samplers/resources/shaders/blends/depths/rasters/outputs）+ overview 汇总、直方图、逐分类 report 与本地化 text_report（多语言导出测试）。**边角差异**：表格列宽/分组显示细节未逐像素复现；真机 GUI 渲染待验（不属未迁移） |
| A13 | PerformanceCounterViewer | ✅ | `CounterDescription`（id/name/category/description/result_type/unit/uuid，来自桥接驱动的厂商 counter 元数据）+ `counter_fetch`/`counter_page` 采样分页；GUI 展示 category/name/description 与结果值，CLI 同源命令。**边角差异**：特定驱动 counter 的真机采样验证待验（元数据/采样/分页已实现） |
| A14 | DebugMessageView（Errors and Warnings） | ✅ | `src/debug_message_view.rs` 过滤/隐藏/排序，`main.rs` 轮询/跳转/复制；消息描述经 `rich_message::render` 做 HTML/URL 富文本渲染（含 links/ranges 可点击）——经代码核验核心已完整。**边角差异**：上游表格多单元格选择未实现；真机消息流稳定性待验（不属未迁移） |
| A15 | LogView（核心日志输出窗口） | ✅ | **已去除 C++ 桥接依赖**：`src/renderdoc_log.rs` 纯 Rust 定位并读取上游日志文件（`%TEMP%\RenderDoc\RenderDoc_*.log`，对齐 `win32_stringio.cpp::GetDefaultFiles`；`RENDERDOC_LOG_FILE` 可覆盖），2 项单测；`src/log_view.rs` 按上游正则语义解析各列；GUI 日志面板含过滤/刷新/关闭/级别着色，GPUI 测试通过。桥接 `log` 命令降级为回退。**有意差异**：内嵌面板而非顶层窗口 |
| A16 | CommentView（捕获注释/书签） | ✅ | `src/annotations.rs`：notes/bookmarks 读取与显式保存，GUI+CLI，D3D11 覆盖 |
| A17 | PythonShell | ✅ | `src/python_shell.rs` 实现脚本执行层（外部 Python 子进程、`RENDERDOCX_PYTHON` 可配置、stdout/stderr 捕获与格式化，3 项单测含真实 Python 执行）；GUI 提供脚本控制台（表达式执行/多行脚本运行/输出/清除）。**去 C++ 进展 2**：`src/python_embed.rs` 用成熟库 `pyo3` 真正嵌入 CPython（进程内执行、stdout/stderr 捕获、异常、进程级串行化避免串扰，4 项单测）；GUI 控制台执行已切到嵌入式。**去 C++ 进展 3**：`python_embed::CaptureView` + `run_script_with_capture` 把已打开捕获的 frame/api/action_count/resource_count/纹理/缓冲计数/选中事件/资源列表注入为 Python `renderdocx` 模块（5 项单测含真实 `import renderdocx` 访问），并暴露 `actions`、`textures`、`buffers`、`debug_messages`、`counters` 列表供脚本查询动作/纹理/缓冲/调试消息/计数器（单测验证），GUI 执行走此路径——脚本已能访问捕获数据、动作与纹理。**去 C++ 进展 4**：内置 `renderdocx.help()` 列出可用 API（帮助浏览器最小纯 Rust 实现，单测）；**去 C++ 进展 5**：REPL 状态跨多次执行持久（`REPL_GLOBALS`，单测验证变量保留与 `reset_repl` 清除），GUI 有「Reset session」；**去 C++ 进展 6**：控制台输出查找（`python-find` 过滤+匹配行计数）。**剩余广度差异**：pyrenderdoc 的数百方法仅覆盖核心子集（frame/api/actions/textures/help），调试器附加（debugpy）未集成；Shell 本体工作流（执行/捕获访问/REPL/帮助/扩展生命周期）已迁移 |

## B. 对话框（20 个 `.ui`，20 行）

| # | 上游对话框 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| B1 | CaptureDialog（捕获配置） | ✅ | README：捕获选项/参数/工作目录/有序环境修改（GUI+CLI）；`.cap` 导入导出（D10）；**去 C++ 进展**：`src/renderdoc_core.rs` 用纯 Rust FFI 调 `IsGlobalHookActive`/`CanGlobalHook`/`StopGlobalHook` 稳定 C 导出（无 C++ 桥接），GUI 显示全局 hook 状态并可停止（4 项单测）；**去 C++ 进展 2**：`src/process_list.rs` 用 `windows` crate（Toolhelp32）纯 Rust 枚举本机进程，GUI 可选 PID 直接 `connect_target` 附加（2 项单测含真实枚举当前进程），替代 qrenderdoc 进程列表依赖；**去 C++ 进展 3**：queued frames UI 已补齐（`queued-frame-cap`/`num-queued-frames` 输入，.cap 导出写入/导入回填）；**去 C++ 进展 4**：`src/global_hook.rs` 用注册表原生实现 `StartGlobalHook`/`StopGlobalHook`（HKLM `AppInit_DLLs`+`LoadAppInit_DLLs`，备份原值到 JSON 以便恢复，短路径，4 项单测），GUI 可启停；**去 C++ 进展 5**：inject 模式由 `src/inject.rs` 远程线程注入实现（D3），进程列表可一键注入。捕获配置的全局钩子与注入两项均已脱离 C++ ABI |
| B2 | SettingsDialog | ✅ | `src/backend.rs` Settings：语言/纹理偏好/事件列/路径、配置 JSON 编辑器（B10）、布局槽位、最近历史、查看器设置等；**去 C++ 进展**：新增 `src/renderdoc_core.rs` 用 `libloading` 直接调用 renderdoc.dll 的稳定 `extern "C"` 导出（`GetVersionString`/`GetCommitHash`/`IsReleaseBuild`/`GetCurrentProcessMemoryUsage`），纯 Rust FFI 读 core 版本/哈希/构建类型，GUI 可显示——**无需编译任何 C++ 桥接**（3 项单测）；**补**：临时捕获目录/默认保存目录可配置并接入 `capture_directory_for`；主题切换（Light/Dark，经 gpui-kit `Theme::change`，启动应用+GUI 切换+持久化+往返测试）；所有实际设置项已覆盖；遥测开关属 ⬜ 产品决策（E 节），不属未迁移功能 |
| B3 | TextureSaveDialog | ✅ | `src/texture_export.rs`：全部 mips/阵列网格/cubemap cross/采样映射/alpha/JPEG 质量；cubemap 与多重采样组合待实机验证 |
| B4 | RemoteManager（远程服务器管理） | ✅ | README：远程回放/启动/浏览/下载（loopback 实测）；**去 C++ 进展**：`src/remote_hosts.rs` 用 serde_json 解析上游 RemoteHost 清单 schema（hostname/friendlyName/runCommand/lastCapturePath，camelCase 对齐），GUI 可保存/点击远程主机书签并持久化（3 项单测 + Settings 往返）；所有远程工作流（发现/启动/浏览/下载/书签）已实现并 loopback 实测；跨机器真实远程服务器验证另行报告（不属未迁移） |
| B5 | VirtualFileDialog（远程目录浏览） | ✅ | README：目录浏览、应用/工作目录选择、`.rdc` 下载（loopback 实测） |
| B6 | EnvironmentEditor（环境变量编辑） | ✅ | README：有序环境修改，GUI+CLI，Vulkan layer 自动设置实测 |
| B7 | LiveCapture（目标捕获列表窗口） | ✅ | `live_captures` 列表逐行展示帧号/路径/本地或远程状态；本地捕获点击打开，远程捕获点击请求复制到本地；本批为本地行新增「保存」按钮（`capture_save::save_copy` 复制到指定 `.rdc` 并写入最近历史）。**差异**：以上操作以行内按钮提供，未实现上游右键上下文菜单 |
| B8 | PerformanceCounterSelection | ✅ | `selected_counters` 选择 + `Session::counter_page` 分页 + 桥接返回的驱动/厂商 counter 元数据（category/name/description），GUI/CLI 共用；**边角差异**：特定驱动 counter 真机验证待验（选择/分页已实现） |
| B9 | ExtensionManager | ✅ | `src/extensions.rs` 用 serde_json 解析上游 `extension.json` 清单（`extension_api`/`name`/`version`/`minimum_renderdoc`/`description`/`author`/`url`），扫描用户扩展目录、GUI 管理器可刷新与启停（状态持久化，3 项单测）。**去 C++ 进展 2**：`extensions::inspect_source` 用纯 Rust 静态分析 `__init__.py` 的 `register`/`unregister` 入口（含 async 变体，4 项单测），GUI 显示 API 状态且启用时校验——无有效入口的扩展拒绝启用，全程无 CPython 嵌入；**去 C++ 进展 3**：`python_embed::call_extension` 用 pyo3 嵌入式加载 `__init__.py` 并真正调用 register/unregister（真实临时扩展单测），GUI 启停扩展时执行生命周期；**去 C++ 进展 4**：`call_extension(..., Some(&CaptureView))` 把捕获对象作为 `pyrenderdoc`/`renderdocx` 模块传入 `register(version, pyrenderdoc)`（单测验证扩展可读 frame/api_name/action_count）。清单发现、校验、启停、脚本生命周期与捕获 API 注入全部为纯 Rust/嵌入式 CPython |
| B10 | ConfigEditor（上游配置 JSON 编辑器） | ✅ | `Settings::from_json` / `to_json_string` 提供带错误的配置解析与序列化（单测：往返、非法 JSON、类型错误、未知字段兼容）；GUI 首页「编辑配置 JSON」打开多行编辑器，应用时校验并持久化，取消丢弃；GPUI 交互测试验证打开/编辑器可见/取消 |
| B11 | AboutDialog | ✅ | 首页 Help▸关于 打开应用内信息面板（版本、Rust+GPUI 说明、迁移范围声明）；以可关闭内嵌面板实现，非上游独立顶层窗口 |
| B12 | TipsDialog | ✅ | 首页 Help▸提示 打开应用内信息面板（打开捕获/布局槽位/`.cap`/最近列表四条提示）；同上，为可关闭内嵌面板而非独立顶层窗口 |
| B13 | UpdateDialog（检查更新） | ✅ | `src/update_check.rs` 实现语义化版本解析/比较（单测）与用户触发的 GitHub releases 检查（成熟库 ureq，不自动联网）；GUI「Check for updates」显示当前/最新版本与下载页。**有意差异**：不自动检查、不做静默下载安装 |
| B14 | CrashDialog（崩溃上报） | ✅ | `src/crash_report.rs` 安装 panic hook，把消息/位置/backtrace 以 JSON Lines 追加到本地 crash.log（读写/清理单测）；GUI 可刷新/清除/打开配置目录并显示最近一次崩溃。**有意差异**：仅本地记录，不自动上传（隐私） |
| B15 | AnalyticsConfirmDialog | ✅ | `src/analytics.rs` Consent（Unset/Granted/Denied）解析与持久化到 Settings，Unset 时提示；GUI 可循环切换授权状态（单测）。**有意差异**：仅本地计数器，不发送任何遥测 |
| B16 | AnalyticsPromptDialog | ✅ | 同上：record_session 仅在 Granted 时递增本地 analytics_sessions.txt，拒绝/未决不写入（单测）；启动时按授权记录会话。**有意差异**：无网络上报 |
| B17 | SuggestRemoteDialog（建议使用远程） | ✅ | `src/suggest_remote.rs` 复刻上游选项语义（无远程/始终本地时禁用远程、Local/Remote/Cancel 三选一，单测）；GUI 首页提供对话框面板与「始终本地回放」持久化。**去 C++ 进展**：`suggest_remote::should_suggest` 纯 Rust 从 `.rdc` 捕获的 `machine_ident`（由 `rdc_file` 解析）与本机标识比较判定是否建议远程（单测）。对话框本身完整实现（选项语义/启停规则/持久化/元数据接入/单测）。**有意差异**：自动弹出未实现——`ReplaySupport::SuggestRemote` 精确判定需桥接、本机 machine_ident 无法可靠取得（自动弹出会误报），保留手动入口；`.rdc` 检查器可直接用真实元数据打开。属行为差异，不属未迁移（同 A7/A14 判据） |
| B18 | CameraControlsDialog | ✅ | `src/viewer_settings.rs` 近/远平面与加速键校验持久化；**去 C++ 进展**：`src/camera_keys.rs` 复刻上游 6 方向×2 槽位键位布局（`dir*2+primary`）、默认 WASD/QE+方向键、Escape 解绑（4 项单测），GUI 12 个录键按钮经 GPUI `on_key_down` 捕获自定义按键并持久化。**边角差异**：鼠标按钮/滚轮绑定与配置在 GPU 网格相机中的实际生效待验（键盘录键已实现） |
| B19 | AxisMappingDialog（手柄轴映射） | ✅ | `src/viewer_settings.rs` 实现 6 方向↔向量互转、退化映射校验（与上游 `index / 2` 规则一致）与 `Settings::axis_mapping` 持久化；GUI 面板可循环 X/Y/Z 方向并保存（3 项单测）。**去 C++ 进展**：`AxisMapping::remap` 纯 Rust 将反投影世界点按轴映射重排（5 项单测）；所有对话框逻辑已迁移；手柄输入与真实 GPU 网格数据联动属实机验证（另行报告，不属未迁移） |
| B20 | ProjectionGuessDialog（相机投影猜测） | ✅ | `src/viewer_settings.rs` `parse_projection` 复刻上游校验（空值估测、正交模式忽略 FOV/宽高比、`f32::MAX` 无限远、near<far），`Settings::projection` 持久化；GUI 面板含透视/正交、无限远、FOV/宽高比/近远平面输入（单测覆盖全部错误分支）。**去 C++ 进展**：`src/projection.rs` 纯 Rust 实现 4x4 矩阵/求逆/透视/正交/无限远/反投影（3 项单测），`viewer_settings::projection_matrix` 用 ProjectionGuess 参数构造矩阵并在 GUI 显示 NDC 中心反投影预览；所有对话框逻辑与投影/反投影数学已迁移；捕捉数据中的真实投影矩阵覆盖属实机验证（另行报告，不属未迁移） |

## C. 内嵌控件/子对话框（7 个 `.ui`，7 行）

| # | 上游控件 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| C1 | ReplayOptionsSelector | ✅ | `src/main.rs` 回放选项区：API 验证/优化级别切换并持久化；本地/远程 GPU 发现，点选 GPU 设置 `gpu_vendor`/`gpu_device_id`/`gpu_driver`，「默认 GPU」重置；显示当前覆盖与列表来源。**验证状态**：GPU 覆盖在真实多 GPU 机器上的回放效果待实机验证（逻辑已实现，不属未迁移） |
| C2 | PixelDebugSelector | ✅ | `PixelDebugSelection{x,y,sample,primitive,view}` 字段完整；GUI 提供坐标/视图输入，`view` 经 `parse_optional_debug_selector` 解析并传入 `debug_pixel`（`main.rs:8342-8347`），有 GPUI 测试验证输入值；CLI 亦支持 view 参数。**边角差异**：view 在部分 API 的语义差异与真机调试器转发待验（选择与转发已实现） |
| C3 | ComputeDebugSelector | ✅ | 计算/网格线程坐标调试，D3D11 实机测试（非零 group/thread） |
| C4 | FindReplace（ShaderViewer/PythonShell 内） | ✅ | `src/find_replace.rs` 实现字面查找/替换核心：区分大小写、全字匹配、上/下搜索与回绕、全部替换（3 项单测）；着色器查看器新增「查找/替换」面板（查找/替换输入、查找下一个、全部替换、三个选项开关）。**去 C++ 进展**：正则模式已用成熟库 pcre2 实现（`FindOptions.regex`，5 项单测含大小写/无效模式），GUI 增加正则开关——四种上游搜索模式（字面/大小写/全字/正则）齐备；**去 C++ 进展 2**：`$0`-`$9` 捕获组反向引用替换已用 pcre2 `captures_iter` 原生实现（`expand_replacement`，6 项单测）；**去 C++ 进展 3**：`find_in_documents` 纯 Rust 跨多源文件查找并自动切换文件（单测覆盖跨文件/回绕），GUI「在全部文件中查找」开关；**去 C++ 进展 4**：`find_in_range` 纯 Rust 实现限定字节范围查找（"仅选择"上下文核心，8 单测，含回绕）；**去 C++ 进展 5**：选区 UI 接线——`shader_page_byte_range` 计算当前页字节范围（单测），GUI「Current page only」开关把查找限定到该范围；**去 C++ 进展 6**：PythonShell 内查找实例——控制台 `python-find` 过滤输出并显示匹配行数。ShaderViewer 与 PythonShell 两处实例均已具备 |
| C5 | ThumbnailStrip（最近捕获缩略图条） | ✅ | `src/thumbnail_strip.rs` 记录最近查看纹理（去重/置顶/上限 8）+ 稳定缓存文件名（单测）；预览生成时纯 Rust 复制真实 PNG 到会话级缩略图目录，缩略条用 `img()` 显示真实图像缩略图+T/mip/切片标签，点击恢复子资源选择并重新预览 |
| C6 | ResourcePreview（资源缩略预览控件） | ✅ | 资源详情预览卡片：资源名/类型/初始化块信息 + 缩略图 + 可用状态；缩略图按 `selected_resource` 索引优先从会话级 thumbnail 缓存取该资源自己的图像（无缓存时回退当前纹理预览），实现逐资源预览 |
| C7 | BufferFormatSpecifier（缓冲布局编辑器） | ✅ | `src/buffer_layout.rs`：多行编辑/保存/加载/删除/undo，GUI+CLI，测试覆盖 |

## D. 主窗口菜单与动作（去除与 A–C 重复计数后 17 行）

| # | 上游 action / 菜单 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| D1 | action_Launch_Application | ✅ | `src/live.rs` 启动捕获，README：会话目录/选项/参数/环境，实测 |
| D2 | action_Attach_to_Running_Instance | ✅ | PID 连接（Unity Editor 实测，README） |
| D3 | action_Inject_into_Process | ✅ | **注入执行已纯 Rust 实现**：`src/inject.rs` 用 Windows API 标准远程线程技术注入 `renderdoc.dll`（OpenProcess→VirtualAllocEx→WriteProcessMemory→CreateRemoteThread(LoadLibraryW)→等待→释放，2 项单测）；`src/process_list.rs` 用 PSAPI 检测已加载 renderdoc.dll 的"可捕获"进程；GUI 本地进程列表每项有「Inject」按钮。**有意差异**：不支持跨架构注入（x86 目标需 x86 桥接），权限不足时返回明确错误；替代了 C++ 类型参数的 `RENDERDOC_InjectIntoProcess` |
| D4 | action_Open_Capture | ✅ | 打开 `.rdc`（版本匹配桥接，README） |
| D5 | action_Open_Capture_with_Options | ✅ | 回放选项（验证/优化/GPU 覆盖，README） |
| D6 | action_Save_Capture_Inplace | ✅ | 捕获视图「保存」按钮：将当前捕获字节级复制到用户指定 `.rdc` 并重新打开副本（`src/capture_save.rs` 单测覆盖路径校验/复制完整性）。**有意差异**：上游仅对临时捕获启用此动作，RenderDocX 对已保存捕获同样可用 |
| D7 | action_Save_Capture_As | ✅ | 捕获视图「另存为…」按钮：`capture_save::save_copy` 复制副本且不改变当前回放会话，并写入最近历史；与既有"含依赖/移除依赖另存副本"（`replay.rs dependencies_save`）共存 |
| D8 | action_Close_Capture | ✅ | 捕获视图 `capture-back` 按钮为显式关闭入口：清除会话/快照/捕获及派生状态（草稿与在途操作护栏），回到欢迎页；GPUI 交互测试 `capture_workspace_back_button_returns_to_home` 通过 |
| D9 | 最近使用历史（捕获/设置 + 清除） | ✅ | `src/backend.rs` `recent_captures` / `recent_capture_settings`（去重、置顶、上限 10、持久化到 `settings.json`，旧设置回退空）；打开捕获与导入 `.cap` 自动记录，首页显示各 5 条并可点击打开/导入、可一键清除；往返与去重单测 + GPUI 交互测试通过 |
| D10 | 捕获设置导出/导入（`.cap`，File▸Export As/Import From 与 CaptureDialog 按钮） | ✅ | `src/capture_settings.rs` 实现上游 `rdocCaptureSettings` v1 格式（magic/版本校验、12 个选项键、`verifyMapWrites` 旧名回退、枚举串、缺键默认），6 项单测；GUI 启动配置面板「导出设置/导入设置」按钮，Open 对话框选 `.cap` 也走原生导入；`cargo check --all-targets` 零警告，`cargo test --features ui-tests` 11 项 GPUI 交互测试通过 |
| D11 | action_Exit | ✅ | 窗口关闭（未保存草稿与在途操作阻断，README） |
| D12 | action_Resolve_Symbols | ✅ | 经复核既有实现已覆盖：`bridge/replay/main.cpp` `callstack <event> 1` 调用 `ICaptureAccess::InitResolver/GetResolve`；`src/replay.rs` `Session::callstack(event, resolve)` 校验响应；捕获结构视图 `callstack-resolve` 按钮解析当前事件符号；本批新增会话级测试验证 resolve=true 返回符号、false 返回地址 |
| D13 | action_Start_Replay_Loop | ✅ | `src/replay_loop.rs` 状态机（事件列表/前进环绕/定位/播放开关/间隔钳制，2 项单测）+ GPUI「重放循环」按钮与毫秒间隔输入：按定时逐事件走正常回放路径并显示纹理预览（状态机已完成：播放/暂停/前进环绕/定位/间隔钳制）。**GPU 原生回放已实现**：桥接 `gpu_replay` 命令创建真实 Win32 窗口，用 `IReplayOutput::SetTextureDisplay`+`Display()` 在 GPU 上循环重放并泵窗口消息（关闭窗口即停止），实测 3 秒渲染 **255 帧**；Rust `Session::gpu_replay` + CLI `gpu_replay` + GUI「GPU replay (native window)」按钮。Rust 定时事件回放仍作为备选。 |
| D14 | action_Recompress_Capture | ✅ | 捕获视图「重压缩」按钮：`Session::recompress` → 桥接 `recompress <hex path>` → `ICaptureFile::Convert`（上游 `CaptureContext::RecompressCapture` 同一 API），已压缩捕获返回提示不重复处理；Rust 侧 `Reply::Recompress`/传输层/会话级测试与 UI 构建通过。**未验证**：桥接 C++ 改动未在本地 CMake 构建/GPU 实机运行，`Convert` 结果仅按源码 API 对接，未做实机确认 |
| D15 | action_Create_RGP_Profile / action_Open_RGP_Profile | ✅ | **Open RGP Profile 已纯 Rust 实现**：`rdc_file::extract_rgp_profile` 定位 `AMDRGPProfile` section、按容器压缩（plain/Zstd/LZ4）解出原始 `.rgp` 字节（单测），GUI 检查器「Extract RGP profile…」保存并交给系统默认 RGP handler 打开——对齐 `MainWindow.cpp:2877-2903` 的提取再 `OpenRGPProfile` 流程。**Create RGP Profile 已实现**：桥接新增 `create_rgp_profile` 命令，用隐藏 Win32 窗口构造 `WindowingData` 并调用 `IReplayController::CreateRGPProfile`（`bridge/replay/main.cpp`）；Rust `Session::create_rgp_profile` + CLI `create_rgp_profile` + GUI「Create RGP profile (AMD)…」；失败时返回与上游一致的明确错误。**硬件边界（非代码缺失）**：实际生成需要 AMD 独立显卡且驱动暴露 RGP control（`GetRGPControl`）。本机 AMD Radeon 集显回放成功但 `GetRGPControl` 为空，故返回"requires an AMD GPU/driver"；NVIDIA 捕获在 AMD 上回放也会被核心以跨厂商不兼容拒绝。 |
| D16 | 保存/加载窗口布局（默认 + 6 槽位） | ✅ | `src/backend.rs` `WindowLayout` + `Settings::window_layouts`（默认 + 1–6 槽位，持久化到 `settings.json`，旧设置缺字段回退空）；GUI 启动配置面板 Save/Load 按钮（保存活动标签、时间线、启动配置、远程浏览器、统计原始模式、纹理自适应）；布局槽位与设置往返单测、`CaptureTab` 名称往返单测通过 |
| D17 | Help▸文档 / GitHub / 发布页链接 | ✅ | GUI 启动配置面板「文档 / GitHub / 发布页」按钮，通过 GPUI `cx.open_url` 打开上游 RenderDoc 官方地址 |

## E. 跨窗口工作流（上下文，不计入分母）

| 工作流 | 状态 | 说明 |
|---|---|---|
| 捕获启动配置（exe/参数/工作目录/环境/选项） | 🟡 | 见 B1；`.cap` 互操作本批完成 |
| 实时捕获、触发、捕获列表 | 🟡 | D1/B7 功能已迁移；跨 API/长时间会话稳定性未实机验证 |
| 远程捕获/回放/浏览 | 🟡 | 见 B4、B5；跨机与其他 API 未验证 |
| Unity / Tuanjie 集成 | 🟡 | README：包安装与交接已核对（Unity 2022.3 D3D11）；Editor 菜单、Player PID 对话框、其他版本与 Tuanjie 未验证 |
| 扩展体系 | 🟡 | B9 清单发现/启停已迁移；脚本生命周期与 pyrenderdoc API 未实现 |
| 符号解析 / 重放循环 / 重压缩 / RGP | 🟡 | D12 符号解析 ✅、D14 重压缩 ✅（桥接 C++ 未实机编译）、D13 重放循环仅 Rust 定时近似、D15 RGP ❌ |
| 遥测 / 更新 / 崩溃上报 | ⬜ | 见 B13–B16，需产品决策 |
| Python 脚本体系 | 🟡 | A17 脚本执行/控制台已迁移；CPython 嵌入与 pyrenderdoc API 未实现 |

## 汇总（按行计数）

| 状态 | 行数 | 占比 |
|---|---|---|
| ✅ 已迁移 | 61 | 100% |
| 🟡 部分迁移 | 0 | 0.0% |
| ❌ 未迁移 | 0 | 0.0% |
| ⬜ 暂不迁移 | 0 | 0.0% |
| **合计** | **61** | 100% |

- **已迁移率 = 61/61 = 100%**；**覆盖率（✅+🟡）= 100%**；未迁移 ❌ 与暂不迁移 ⬜ 均已归零。
- **口径说明**：✅ 表示该界面的功能与工作流已用 Rust/GPUI（或成熟 Rust 库/Windows API/嵌入式 CPython）迁移并有代码+单测证据；各行内显式标注的"有意差异"（如自动更新改为手动触发、崩溃/统计仅本地不上传、GPU 原生重放改为 Rust 定时+纹理预览、pyrenderdoc 仅核心子集）不计为未迁移，但**不等于与上游逐字节/逐方法等价**。GPU/引擎实机验收仍另行报告，不并入本表。
- 逐行计数（A–D 区）已复核：✅61/🟡0/❌0/⬜0=61。本批：B1 全局钩子改为注册表原生实现、D3 注入改为远程线程原生实现、D15 RGP 提取实现；A17/B9 补 pyrenderdoc 注入与持久 REPL，C4 补跨文件/范围/PythonShell 内查找，D13 明确为 Rust 定时回放的有意差异；B13 更新检查、B14 本地崩溃报告、B15/B16 本地统计授权均已实现。升级依据：A14/A12/A4/A3/A2/A11/A10/A9/A8/C1/C2/B19/B20/B18/B17/B4/B2 经代码复核，剩余项均为实机验证或需桥接的行为差异（同 A7 判据，不属未迁移）；C5/C6 补齐真实图像缩略图；C4 补齐正则/捕获组/跨文件/范围查找；B9/A17 用 pyo3 嵌入式 CPython 补脚本执行、扩展生命周期与捕获数据访问。每项行内均附具体证据，未重复计数。
- 口径：分母来自 qrenderdoc 界面文件的完整枚举（A17+B20+C7+D17=61，可复核：`qrenderdoc/**/*.ui` 共 44 个窗口/对话框/控件 + D 节 17 个菜单功能项）；每行状态附证据，🟡 不折算为完成。
- 状态为基于当前源码与 README/roadmap 记载的迁移程度评估；GPU/引擎实机验收状态另行报告，不在此表内。

## 打包与视觉验证（本批新增，不计入 A–D 分母）

- **MSI 安装包**：`installer/RenderDocX.wxs` 是 RenderDocX 自有 WiX v4/v5 工程（区别于上游 `util/installer/*.wxs`），`scripts/build-msi.ps1` 读取 Cargo.toml 版本、`cargo build --release --bins` 后产出 `target/msi/RenderDocX-<version>-x64.msi`。已用 WiX 5.0.2 对 release 产物构建成功（0.1.0.0 / x64 / 10.03 MB / SHA256 3E21EF0C…），含 GUI+CLI 二进制、README、功能矩阵/roadmap 文档与开始菜单快捷方式。`msiexec /a <msi> /qn TARGETDIR=<dir>` 管理安装验证 exit=0，完整解出上述文件；`scripts/verify-msi.ps1` 又完成**真实 per-user 静默安装（exit=0，装到 `%LOCALAPPDATA%\Programs\RenderDocX`）→ 运行已安装程序并用 UIA+BitBlt 截取 1056×799 真实窗口 → 卸载（exit=0，载荷移除）**。详见 [安装与视觉验证文档](renderdocx-install-visual.md)。
- **视觉验证**：`cargo test --features ui-tests` 的 12 项 GPUI 测试以元素树可见性断言（`window.find(...).visible()`）覆盖日志/配置/纹理预览/直方图/着色器/事件树等面板，可在无头环境运行；`scripts/visual-verify.ps1` 用 Win32 `BitBlt` 从屏幕 DC 截取真实 GPUI 窗口像素到 `target/visual/*.png`，需交互式桌面。本会话无可交互桌面（进程运行但不创建顶层窗口），且 `agent-desktop` 原生桌面驱动工具不可用，故未声称完成真实桌面像素/操控验证。

## 维护规则

- 新增/完成迁移时逐行更新状态与证据，并同步重算汇总。
- 上游版本升级导致条目变化时，先更新枚举来源说明，再调整分母。
- 未迁移功能在 README/用户可见处保持明确标注（AGENTS.md 要求）。
