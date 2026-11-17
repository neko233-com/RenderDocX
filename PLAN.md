# RenderDocX 计划（PLAN）

## 目标

对标竞品 [RenderDoc](https://github.com/baldurk/renderdoc)，目标是把竞品翻译成 **Rust + GPUI 原生渲染** 的版本：

- 用 Rust 重写 `qrenderdoc` GUI：界面由 GPUI 原生窗口、原生控件和原生渲染构成，不依赖 Qt，也不以调用 Qt 前端冒充已完成的迁移。
- **完全遵循 RenderDoc 规范**：捕获与回放行为、公开 API、`.rdc` 文件格式、资源/事件/管线状态语义一律以上游对应版本的源码和文档为准，不擅自偏离。
- 当前只交付 **Windows** 平台：实现、测试、打包与验收都以 Windows 为准。
- **原生多语言，当前只支持简体中文和英语**：启动检测系统语言并支持手动切换，未知语言回退英语；日语、韩语已从实现中移除（旧设置值回退为自动检测），`Language` 枚举保留扩展点，不纳入本阶段验收。
- 捕获与回放继续运行在匹配版本的 RenderDoc 核心上（版本与构建提交双重校验的桥接层），核心语义与上游保持一致；条件成熟时再逐步转为 Rust。

硬性约束的完整表述见 [AGENTS.md](AGENTS.md)；两份文件冲突时以 AGENTS.md 为准。

## 对标基线

- 竞品/上游：RenderDoc。本仓库源码快照自述版本 1.48，文件级比对到 upstream `v1.x` commit [`5ad608809c9028da9f5c5082397cb6afe9c8dd17`](https://github.com/baldurk/renderdoc/commit/5ad608809c9028da9f5c5082397cb6afe9c8dd17)，比对口径见 README 的 "Source and compatibility"。
- 对标对象：`qrenderdoc` 的全部窗口、对话框与工作流（事件浏览器、纹理查看器、管线状态、资源、网格、顶点输入、着色器调试与编辑、像素历史、目标控制、远程捕获与回放、设置、快捷键、无障碍等），以及上游行为语义（捕获、回放、`.rdc` 格式、API 语义）。
- 对标方法：以上游源码与实际运行为唯一准绳。每个功能标记为**已迁移 / 部分迁移 / 未迁移**，并附验证证据；不得凭印象宣称兼容。

## 当前状态

早期原型：工作区可启动与发现本地 RenderDoc 目标、触发捕获并打开 `.rdc`，已实现原生事件浏览器、纹理预览与导出、常见管线状态、缓冲查看、网格与顶点输入、着色器调试等初始视图，以及面向 Agent 的 CLI；部分能力仍需 `qrenderdoc` 兜底。逐项能力、已验证与未验证边界见 [README.md](README.md) 与 [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md)。

进度百分比在“对标清单（完整分母）”建立之前不计算，基准未建立时明确说明，禁止粗略估算（见 AGENTS.md 进度汇报要求）。

## 阶段计划

按上游窗口和工作流批量迁移、集成：先推进全部功能实现，再集中执行完整自动化与验收。详细剩余工作以 roadmap 的编号条目为准，本计划只规定推进顺序：

| 阶段 | 内容 | 对应 roadmap 条目 |
| --- | --- | --- |
| P0 建立分母 | 从 `qrenderdoc` 源码枚举全部窗口、对话框与工作流，形成可核对的功能对标清单，作为后续进度的完整分母 | 前置 |
| P1 核心与桥接 | 版本校验的 replay helper、生命周期、打包、可复现的核心构建 | 1 |
| P2 主体迁移 | 目标枚举与控制、捕获配置与进度、事件浏览器、纹理/网格/管线状态/资源/着色器调试等全部上游窗口 | 2、3 |
| P3 生态与远程 | Unity/Tuanjie 工作流、远程捕获与回放、扩展 | 4 |
| P4 原生化完善 | 中英文案全覆盖、键盘导航、无障碍、设置、Windows 打包 | 5 |
| P5 验收 | 协议、UI 交互、真实捕获、回放、GPU、回归的完整自动化与实机验收 | 6 |

## 验收与汇报口径

- 构建、自动化检查、真实 GPU/引擎验收三类结果分别报告，不得互相替代。
- 只有对标清单建立、且每项具备可核实证据后才汇报进度百分比；汇报时说明统计口径与剩余工作。
- 未验证的兼容性必须显式标注；编译通过与模拟测试不等于功能达成。
- 每批工作完成后，按 AGENTS.md 的要求汇报本批进度与整体进度。

## 参考

- [AGENTS.md](AGENTS.md) — 开发原则与硬性约束
- [README.md](README.md) — 当前能力与验证方式
- [docs/renderdocx-roadmap.md](docs/renderdocx-roadmap.md) — 迁移边界与剩余工作
- [docs/agent-cli.md](docs/agent-cli.md) — Agent CLI 协议
- 上游项目：https://github.com/baldurk/renderdoc
