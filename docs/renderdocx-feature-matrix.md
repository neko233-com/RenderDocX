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
| A2 | EventBrowser | 🟡 | `src/event_*.rs`、README：过滤器/分组/书签/时长/导出已实现；上游其余过滤函数与 action 属性未迁移 |
| A3 | TextureViewer | 🟡 | `src/texture_view.rs`、`texture_overlay.rs`、`texture_export.rs`、像素历史/统计/自定义着色器已实现；完整交互式上游纹理显示、任意 descriptor swizzle 未迁移 |
| A4 | PipelineState（base + D3D11/D3D12/GL/Vulkan 5 个 `.ui`） | 🟡 | `src/replay.rs` PipeState（上游公共实现）、`vertex_input.rs`、`sampler_view.rs`；分 API 完整视图与更广的管线检查未完成 |
| A5 | APIInspector | ✅ | `main.rs` API calls 列表（`api-event-{index}`，上游 `APIEvent` 名称 + EID）可点击回放所选事件；参数页 `structure` 展示 chunk 子节点；`event_parameters::format_event` 生成上游风格 `Call(Top/ology=4, ...)` 格式化调用摘要并由 `event-format` 展示；GPUI 交互测试 `clicking_an_action_replays_its_exact_event` 验证列表→回放→参数→格式化全链路 |
| A6 | BufferViewer | 🟡 | `src/buffer_*.rs`：分页/搜索/命名布局/CSV 导出已实现；上游其余打包规则与语言特性未完成 |
| A7 | ShaderViewer | ✅ | `src/shader_source.rs` 分页源码、`shader_export.rs` 导出、`custom_shaders.rs` 替换/草稿，Find/Replace 面板（C4）已接入源码视图；外部处理器配置导入/移除/运行 GUI 已实现（`shader-processor-import` 等）。**验证状态**：外部处理器真实执行链仍待 GPU/工具链实机验收，不属未迁移功能 |
| A8 | ShaderMessageViewer | 🟡 | `src/shader_messages.rs`：分页/过滤/导出/跳转/调试衔接；HTML/URL 处理、多单元格选择、实机 GUI 验证未完成 |
| A9 | PixelHistoryView | 🟡 | README：片段列表/前后值/重放/统计直方图已实现；后端无法检查像素时的空结果语义与其余 API 待验证 |
| A10 | ResourceInspector | 🟡 | README「Resources」：列表/过滤/用法导航已实现；上游资源详情面板（类型/尺寸/格式明细）未迁移 |
| A11 | DescriptorViewer | 🟡 | `src/replay.rs` descriptors：分页/类型/范围/跳转；可变长度 store 枚举、完整呈现、跨 API 验证未完成 |
| A12 | StatisticsViewer | 🟡 | `src/frame_statistics.rs`：概览/分类/直方图/本地化导出；概览与报告未复现全部上游格式细节，实机 GUI 验证未完成 |
| A13 | PerformanceCounterViewer | 🟡 | README：GPU counter 选择/采样/分页（GUI+CLI）；驱动/厂商 counter 与完整上游 counter 工作流未完成 |
| A14 | DebugMessageView（Errors and Warnings） | 🟡 | `src/debug_message_view.rs`：轮询/过滤/隐藏/跳转/复制；HTML/URL、多单元格选择、实机验证未完成 |
| A15 | LogView（核心日志输出窗口） | ✅ | **已去除 C++ 桥接依赖**：`src/renderdoc_log.rs` 纯 Rust 定位并读取上游日志文件（`%TEMP%\RenderDoc\RenderDoc_*.log`，对齐 `win32_stringio.cpp::GetDefaultFiles`；`RENDERDOC_LOG_FILE` 可覆盖），2 项单测；`src/log_view.rs` 按上游正则语义解析各列；GUI 日志面板含过滤/刷新/关闭/级别着色，GPUI 测试通过。桥接 `log` 命令降级为回退。**有意差异**：内嵌面板而非顶层窗口 |
| A16 | CommentView（捕获注释/书签） | ✅ | `src/annotations.rs`：notes/bookmarks 读取与显式保存，GUI+CLI，D3D11 覆盖 |
| A17 | PythonShell | 🟡 | `src/python_shell.rs` 实现脚本执行层（外部 Python 子进程、`RENDERDOCX_PYTHON` 可配置、stdout/stderr 捕获与格式化，3 项单测含真实 Python 执行）；GUI 提供脚本控制台（表达式执行/多行脚本运行/输出/清除）。**未完成**：未嵌入 CPython 与 `pyrenderdoc` 绑定，脚本无法访问已打开捕获、无 REPL 状态/调试器附加/帮助浏览器/项目浏览器 |

## B. 对话框（20 个 `.ui`，20 行）

| # | 上游对话框 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| B1 | CaptureDialog（捕获配置） | 🟡 | README：捕获选项/参数/工作目录/有序环境修改（GUI+CLI）；`.cap` 导入导出（D10）；**去 C++ 进展**：`src/renderdoc_core.rs` 用纯 Rust FFI 调 `IsGlobalHookActive`/`CanGlobalHook`/`StopGlobalHook` 稳定 C 导出（无 C++ 桥接），GUI 显示全局 hook 状态并可停止（4 项单测）；**去 C++ 进展 2**：`src/process_list.rs` 用 `windows` crate（Toolhelp32）纯 Rust 枚举本机进程，GUI 可选 PID 直接 `connect_target` 附加（2 项单测含真实枚举当前进程），替代 qrenderdoc 进程列表依赖；**剩余缺口**：`StartGlobalHook`（参数含 C++ `rdcstr&`/`CaptureOptions&`，ABI 不安全不可 FFI）、inject 模式、queued frames UI |
| B2 | SettingsDialog | 🟡 | `src/backend.rs` Settings：语言/纹理偏好/事件列/路径、配置 JSON 编辑器（B10）、布局槽位、最近历史、查看器设置等；**去 C++ 进展**：新增 `src/renderdoc_core.rs` 用 `libloading` 直接调用 renderdoc.dll 的稳定 `extern "C"` 导出（`GetVersionString`/`GetCommitHash`/`IsReleaseBuild`/`GetCurrentProcessMemoryUsage`），纯 Rust FFI 读 core 版本/哈希/构建类型，GUI 可显示——**无需编译任何 C++ 桥接**（3 项单测）；上游设置项（临时目录、主题、遥测开关等）未覆盖 |
| B3 | TextureSaveDialog | ✅ | `src/texture_export.rs`：全部 mips/阵列网格/cubemap cross/采样映射/alpha/JPEG 质量；cubemap 与多重采样组合待实机验证 |
| B4 | RemoteManager（远程服务器管理） | 🟡 | README：远程回放/启动/浏览/下载（loopback 实测）；远程目标发现与部分远程工作流未完成 |
| B5 | VirtualFileDialog（远程目录浏览） | ✅ | README：目录浏览、应用/工作目录选择、`.rdc` 下载（loopback 实测） |
| B6 | EnvironmentEditor（环境变量编辑） | ✅ | README：有序环境修改，GUI+CLI，Vulkan layer 自动设置实测 |
| B7 | LiveCapture（目标捕获列表窗口） | ✅ | `live_captures` 列表逐行展示帧号/路径/本地或远程状态；本地捕获点击打开，远程捕获点击请求复制到本地；本批为本地行新增「保存」按钮（`capture_save::save_copy` 复制到指定 `.rdc` 并写入最近历史）。**差异**：以上操作以行内按钮提供，未实现上游右键上下文菜单 |
| B8 | PerformanceCounterSelection | 🟡 | counter 选择与分页已实现；与 B13 同源的完整工作流未完成 |
| B9 | ExtensionManager | ❌ | `src/` 无扩展体系 |
| B10 | ConfigEditor（上游配置 JSON 编辑器） | ✅ | `Settings::from_json` / `to_json_string` 提供带错误的配置解析与序列化（单测：往返、非法 JSON、类型错误、未知字段兼容）；GUI 首页「编辑配置 JSON」打开多行编辑器，应用时校验并持久化，取消丢弃；GPUI 交互测试验证打开/编辑器可见/取消 |
| B11 | AboutDialog | ✅ | 首页 Help▸关于 打开应用内信息面板（版本、Rust+GPUI 说明、迁移范围声明）；以可关闭内嵌面板实现，非上游独立顶层窗口 |
| B12 | TipsDialog | ✅ | 首页 Help▸提示 打开应用内信息面板（打开捕获/布局槽位/`.cap`/最近列表四条提示）；同上，为可关闭内嵌面板而非独立顶层窗口 |
| B13 | UpdateDialog（检查更新） | ⬜ | 暂不迁移：上游联网更新检查，需产品决策 |
| B14 | CrashDialog（崩溃上报） | ⬜ | 暂不迁移：上游崩溃上报，需产品决策 |
| B15 | AnalyticsConfirmDialog | ⬜ | 暂不迁移：上游统计授权，需产品决策 |
| B16 | AnalyticsPromptDialog | ⬜ | 暂不迁移：上游统计授权，需产品决策 |
| B17 | SuggestRemoteDialog（建议使用远程） | 🟡 | `src/suggest_remote.rs` 复刻上游选项语义（无远程/始终本地时禁用远程、Local/Remote/Cancel 三选一，单测）；GUI 首页提供对话框面板与「始终本地回放」持久化。**未完成**：桥接未暴露上游 `ReplaySupport::SuggestRemote` 与机器标识，打开捕获时不会自动弹出，需手动从首页入口打开 |
| B18 | CameraControlsDialog | 🟡 | `src/viewer_settings.rs` 实现近/远平面与加速键（Shift/Alt/Ctrl/无）校验及 `Settings::camera` 持久化，GUI 面板可编辑保存。**未完成**：鼠标/键盘按键捕获重绑定（上游 `setKey` 录键流程）与配置在网格相机中的实际生效未实现 |
| B19 | AxisMappingDialog（手柄轴映射） | 🟡 | `src/viewer_settings.rs` 实现 6 方向↔向量互转、退化映射校验（与上游 `index / 2` 规则一致）与 `Settings::axis_mapping` 持久化；GUI 面板可循环 X/Y/Z 方向并保存（3 项单测）。**未完成**：映射尚未接入网格顶点反投影/手柄输入 |
| B20 | ProjectionGuessDialog（相机投影猜测） | 🟡 | `src/viewer_settings.rs` `parse_projection` 复刻上游校验（空值估测、正交模式忽略 FOV/宽高比、`f32::MAX` 无限远、near<far），`Settings::projection` 持久化；GUI 面板含透视/正交、无限远、FOV/宽高比/近远平面输入（单测覆盖全部错误分支）。**未完成**：参数尚未传入网格 VS Output 反投影计算 |

## C. 内嵌控件/子对话框（7 个 `.ui`，7 行）

| # | 上游控件 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| C1 | ReplayOptionsSelector | 🟡 | README：API 验证/优化级别/GPU 覆盖选择；强制 GPU 选择未验证 |
| C2 | PixelDebugSelector | 🟡 | 像素调试坐标/视图选择器已实现；view 未指定、GPUI selector 转发未实测 |
| C3 | ComputeDebugSelector | ✅ | 计算/网格线程坐标调试，D3D11 实机测试（非零 group/thread） |
| C4 | FindReplace（ShaderViewer/PythonShell 内） | 🟡 | `src/find_replace.rs` 实现字面查找/替换核心：区分大小写、全字匹配、上/下搜索与回绕、全部替换（3 项单测）；着色器查看器新增「查找/替换」面板（查找/替换输入、查找下一个、全部替换、三个选项开关）。**未完成**：上游正则模式、查找上下文（选择/全部文件）、PythonShell 内实例（PythonShell 本身未迁移） |
| C5 | ThumbnailStrip（最近捕获缩略图条） | 🟡 | `src/thumbnail_strip.rs` 记录最近查看纹理（去重/置顶/上限 8）并有单测；纹理视图新增可点击缩略条（T/mip/切片标签），点击恢复该子资源选择并重新预览。**未完成**：上游缩略条显示真实纹理图像，RenderDocX 当前为文本标签条 |
| C6 | ResourcePreview（资源缩略预览控件） | 🟡 | 资源详情新增预览卡片：资源名/类型/初始化块信息 + 当前纹理预览 PNG 缩略图 + 可用状态提示。**未完成**：未按每个资源单独生成缩略图（复用当前纹理预览），与上游逐资源预览不同 |
| C7 | BufferFormatSpecifier（缓冲布局编辑器） | ✅ | `src/buffer_layout.rs`：多行编辑/保存/加载/删除/undo，GUI+CLI，测试覆盖 |

## D. 主窗口菜单与动作（去除与 A–C 重复计数后 17 行）

| # | 上游 action / 菜单 | 状态 | 证据 / 缺口 |
|---|---|---|---|
| D1 | action_Launch_Application | ✅ | `src/live.rs` 启动捕获，README：会话目录/选项/参数/环境，实测 |
| D2 | action_Attach_to_Running_Instance | ✅ | PID 连接（Unity Editor 实测，README） |
| D3 | action_Inject_into_Process | ❌ | `src/` 无进程注入 |
| D4 | action_Open_Capture | ✅ | 打开 `.rdc`（版本匹配桥接，README） |
| D5 | action_Open_Capture_with_Options | ✅ | 回放选项（验证/优化/GPU 覆盖，README） |
| D6 | action_Save_Capture_Inplace | ✅ | 捕获视图「保存」按钮：将当前捕获字节级复制到用户指定 `.rdc` 并重新打开副本（`src/capture_save.rs` 单测覆盖路径校验/复制完整性）。**有意差异**：上游仅对临时捕获启用此动作，RenderDocX 对已保存捕获同样可用 |
| D7 | action_Save_Capture_As | ✅ | 捕获视图「另存为…」按钮：`capture_save::save_copy` 复制副本且不改变当前回放会话，并写入最近历史；与既有"含依赖/移除依赖另存副本"（`replay.rs dependencies_save`）共存 |
| D8 | action_Close_Capture | ✅ | 捕获视图 `capture-back` 按钮为显式关闭入口：清除会话/快照/捕获及派生状态（草稿与在途操作护栏），回到欢迎页；GPUI 交互测试 `capture_workspace_back_button_returns_to_home` 通过 |
| D9 | 最近使用历史（捕获/设置 + 清除） | ✅ | `src/backend.rs` `recent_captures` / `recent_capture_settings`（去重、置顶、上限 10、持久化到 `settings.json`，旧设置回退空）；打开捕获与导入 `.cap` 自动记录，首页显示各 5 条并可点击打开/导入、可一键清除；往返与去重单测 + GPUI 交互测试通过 |
| D10 | 捕获设置导出/导入（`.cap`，File▸Export As/Import From 与 CaptureDialog 按钮） | ✅ | `src/capture_settings.rs` 实现上游 `rdocCaptureSettings` v1 格式（magic/版本校验、12 个选项键、`verifyMapWrites` 旧名回退、枚举串、缺键默认），6 项单测；GUI 启动配置面板「导出设置/导入设置」按钮，Open 对话框选 `.cap` 也走原生导入；`cargo check --all-targets` 零警告，`cargo test --features ui-tests` 11 项 GPUI 交互测试通过 |
| D11 | action_Exit | ✅ | 窗口关闭（未保存草稿与在途操作阻断，README） |
| D12 | action_Resolve_Symbols | ✅ | 经复核既有实现已覆盖：`bridge/replay/main.cpp` `callstack <event> 1` 调用 `ICaptureAccess::InitResolver/GetResolve`；`src/replay.rs` `Session::callstack(event, resolve)` 校验响应；捕获结构视图 `callstack-resolve` 按钮解析当前事件符号；本批新增会话级测试验证 resolve=true 返回符号、false 返回地址 |
| D13 | action_Start_Replay_Loop | 🟡 | `src/replay_loop.rs` 状态机（事件列表/前进环绕/定位/播放开关/间隔钳制，2 项单测）+ GPUI「重放循环」按钮与毫秒间隔输入：按定时逐事件走正常回放路径并显示纹理预览。**有意差异/未完成**：上游由桥接 `IReplayController::ReplayLoop(WindowingData, ResourceId)` 在原生窗口内做 GPU 逐帧重放，RenderDocX 没有对应桥接窗口句柄，当前仅 Rust 侧定时近似，未做 GPU 实机验证 |
| D14 | action_Recompress_Capture | ✅ | 捕获视图「重压缩」按钮：`Session::recompress` → 桥接 `recompress <hex path>` → `ICaptureFile::Convert`（上游 `CaptureContext::RecompressCapture` 同一 API），已压缩捕获返回提示不重复处理；Rust 侧 `Reply::Recompress`/传输层/会话级测试与 UI 构建通过。**未验证**：桥接 C++ 改动未在本地 CMake 构建/GPU 实机运行，`Convert` 结果仅按源码 API 对接，未做实机确认 |
| D15 | action_Create_RGP_Profile / action_Open_RGP_Profile | ❌ | `qrenderdoc/Code/RGPInterop.cpp` 未迁移（依赖 AMD RGP） |
| D16 | 保存/加载窗口布局（默认 + 6 槽位） | ✅ | `src/backend.rs` `WindowLayout` + `Settings::window_layouts`（默认 + 1–6 槽位，持久化到 `settings.json`，旧设置缺字段回退空）；GUI 启动配置面板 Save/Load 按钮（保存活动标签、时间线、启动配置、远程浏览器、统计原始模式、纹理自适应）；布局槽位与设置往返单测、`CaptureTab` 名称往返单测通过 |
| D17 | Help▸文档 / GitHub / 发布页链接 | ✅ | GUI 启动配置面板「文档 / GitHub / 发布页」按钮，通过 GPUI `cx.open_url` 打开上游 RenderDoc 官方地址 |

## E. 跨窗口工作流（上下文，不计入分母）

| 工作流 | 状态 | 说明 |
|---|---|---|
| 捕获启动配置（exe/参数/工作目录/环境/选项） | 🟡 | 见 B1；`.cap` 互操作本批完成 |
| 实时捕获、触发、捕获列表 | 🟡 | D1/B7 功能已迁移；跨 API/长时间会话稳定性未实机验证 |
| 远程捕获/回放/浏览 | 🟡 | 见 B4、B5；跨机与其他 API 未验证 |
| Unity / Tuanjie 集成 | 🟡 | README：包安装与交接已核对（Unity 2022.3 D3D11）；Editor 菜单、Player PID 对话框、其他版本与 Tuanjie 未验证 |
| 扩展体系 | ❌ | 见 B9 |
| 符号解析 / 重放循环 / 重压缩 / RGP | 🟡 | D12 符号解析 ✅、D14 重压缩 ✅（桥接 C++ 未实机编译）、D13 重放循环仅 Rust 定时近似、D15 RGP ❌ |
| 遥测 / 更新 / 崩溃上报 | ⬜ | 见 B13–B16，需产品决策 |
| Python 脚本体系 | 🟡 | A17 脚本执行/控制台已迁移；CPython 嵌入与 pyrenderdoc API 未实现 |

## 汇总（按行计数）

| 状态 | 行数 | 占比 |
|---|---|---|
| ✅ 已迁移 | 28 | 45.9% |
| 🟡 部分迁移 | 26 | 42.6% |
| ❌ 未迁移 | 3 | 4.9% |
| ⬜ 暂不迁移 | 4 | 6.6% |
| **合计** | **61** | 100% |

- **已迁移率 = 28/61 ≈ 45.9%**；**覆盖率（✅+🟡）= 54/61 ≈ 88.5%**。
- 本批变更：A17（PythonShell）由 ❌ 转为 🟡（脚本执行/控制台外壳已迁移，CPython 嵌入与 pyrenderdoc API 未实现）；此前 A1、A7、B7、A5、B17、C5/C6、C4、B10、B18/B19/B20、D13、D14、D6/D7、A15/D12、D9/B11/B12、D8/D16/D17、D10 已计入此前口径，未重复计数。
- 口径：分母来自 qrenderdoc 界面文件的完整枚举（A17+B20+C7+D17=61，可复核：`qrenderdoc/**/*.ui` 共 44 个窗口/对话框/控件 + D 节 17 个菜单功能项）；每行状态附证据，🟡 不折算为完成。
- 状态为基于当前源码与 README/roadmap 记载的迁移程度评估；GPU/引擎实机验收状态另行报告，不在此表内。

## 维护规则

- 新增/完成迁移时逐行更新状态与证据，并同步重算汇总。
- 上游版本升级导致条目变化时，先更新枚举来源说明，再调整分母。
- 未迁移功能在 README/用户可见处保持明确标注（AGENTS.md 要求）。
