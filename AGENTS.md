# AGENTS.md

## 强制执行规范：主要目标优先（所有项目通用；2026-10-17 更新）

本节同时约束 GPT 与 Codex，必须落实到实际决策和操作。它取代较早执行规范中与本节冲突的要求；业务需求以用户最新有效决定为准。GPT 每份给 Codex 的任务必须先写结合当前事实的具体任务正文，再完整附带本节；不能仅在末尾追加通用条款，不能用“同上”或文件链接代替。

1. **主要目标是第一判断依据。** 先明确项目最终要交付的业务结果、本阶段主要任务、本次操作主要任务和最后有证据的成功点。所有操作以完成本次任务、推进本阶段交付为首要要求；开发、修复、测试、门槛、整理和报告都须说明对该结果的实际作用。不能将“测试全绿”“零告警”“治理完成”自行替代为主任务，也不擅自增加或降低用户既定要求。
2. **先写具体任务，再按目标执行。** GPT 派发的正文必须写清“本阶段主要任务；本次操作主要任务及必要原因；已有成果与复用线索；直接推进交付的操作；本次验证与阻断依据；完成条件”。结合实际需求、已知代码入口和当前证据填写，未知本机事实说明待定位，不能编造路径、版本或结论。测试和门槛是否需要须在本次正文中判断，不能让通用条款替代决策。开始独立工作或转入新问题前简明说明目标和必要原因，同一目的下连续操作合并说明；根据已有需求自行确认并在授权内继续，不逐步等待用户批准。
3. **开发前必须查重并优先复用。** 开发或修复前，先明确本次操作与主要目标的关系，再沿相关功能入口、函数、调用链和已有功能索引，查找当前项目、已知共享模块及已安装依赖中的可复用实现；不能只因名称不同或一次关键词搜索未命中，就认定功能不存在。已有实现满足需求就直接复用；只缺部分能力，优先扩展或修复原实现；确认没有合适实现时才新增必要代码，禁止复制原逻辑另建重复功能或并行处理链路。用实际代码路径、函数或接口及一句理由说明选择复用、扩展或新增的依据；未找到时说明已查的相关范围，不假报不存在。查重以足够判断本次方案为限，结论明确后继续开发，不扩大为全仓审计、专项报告、固定测试、再次审批或每次全网寻找开源项目。
4. **默认不追加测试，只补必要证据。** 已有证据足以确认当前交付时不新增、不重跑测试。确需验证时，执行方先说明尚未确认的交付要求、改动引入的具体风险或真实数据/权限边界，解释已有证据为何不足，再选择足以消除该疑问的最小有效检查。不能以“更稳妥”“规范要求”代替必要性依据；不为规则文字新建专项测试，不因换会话或无关失败重跑全仓。也不能用少测为由把尚未证实的结果宣称成功。
5. **默认不增设门槛，只处理真实阻碍。** 测试有必要，不代表它的任何失败都能阻断整个项目。只有有具体依据证明当前交付无法完成、达不到用户既定标准、破坏相关必要能力或越过真实权限/数据边界，才能阻断对应步骤；说明影响什么结果、为何必须先解决。无关旧错误、可选优化、测试夹具或提示差异按实际影响简记，继续可完成的主任务。不得自行设置全仓全绿、固定测试数量、全部警告清零、重复审计或额外审批条件；执行中新出现的必要依赖也须说明依据，不能借机扩大任务。
6. **旧规则也要按主要目标核对。** 不能仅因旧文档、测试清单、Hook 或 CI 已经存在，就认定其要求必然必要。发现历史要求与最新目标冲突时，在职责和授权内修正相关规则；自动检查的失败应说明实际影响，不能静默跳过、伪造通过或绕过真实保护。已获授权不反复索取；确实缺少必要权限、费用、资源或关键决定时，先完成不受影响的部分再说明具体缺口。
7. **按分工推进，并对方案负责。** GPT 先修改 GitHub 项目代码和规则；Codex 负责所交办的本机运行、必要验证，以及明确授权的本机配置、治理脚本或本地代码修改，返回实际差异、版本和必要证据交 GPT 确认。默认业务源码仍由 GPT 修改。GPT 派发前须删去不能说明必要性的步骤；发现用户建议、旧方案或自己的安排与目标冲突，应主动说明具体影响并提出更直接的方案，不能只附和或机械转交。复用已有实现和状态，保护已验收能力及用户工作，不重复开发、不清空无关改动、不另建同功能链路。
8. **达到本次完成条件就交付。** 有职责和授权内的直接下一步就连续执行；本次结果达到用户既定要求、必要证据足够后即返回交付，不能自行扩展测试、修复或治理。GPT 按任务结果、实际差异和必要证据复核，不以重复运行或追加流程代替判断；若证据不足，指出影响哪项交付的具体缺口。报告先说实际结果，再说必要证据和剩余问题，区分已实现、已实测、GPT 已确认与用户已验收。沿用已有记录，不为证明遵守规范新增表格、审批、治理系统或停工条件。

> 本文件是本项目 AI 编码工具的**单一真相源**。其它工具入口若存在，只应引用本文件，不要重复维护规则。
> 分阶段 backlog 已迁移到 `.trellis/tasks/`，每个任务的 PRD 与研究资料以 Trellis task 为准。

EcoPaste 是跨平台剪贴板管理器，采用 Rust-First 的 Tauri 架构。

## 快速原则

- **Rust-First**：业务、系统能力、数据库与持久化优先放 Rust；前端只做展示与交互。
- **仅支持 macOS + Windows**：不要新增 Linux 代码、依赖、构建产物或文档承诺。
- **已发布版本按发布数据处理**：数据结构、配置格式、默认值和 migration 变更必须有明确迁移策略，不再直接覆盖已发布数据契约。
- **主动演进当前项目**：实现新能力时以当前代码、产品需求和平台约束为准，把当前仓库作为唯一实现基线。
- **尊重 dirty worktree**：不要回滚或覆盖非本轮改动；需要动到已修改文件时先读清楚。
- **提交与推送分支策略**：沿用用户已经确定的开发分支 / Draft PR（当前为 PR #3、`feature/clipboard-priority-pin-panel`），在既有授权内提交和推送，不反复询问。若仅处于 `master`，自动创建工作分支；只有无法从现有任务判断目标分支或存在必须由用户决定的工作冲突时才澄清。未经授权不合并或覆盖稳定版本。

## 技术栈

| 维度 | 选型                                            |
| ---- | ----------------------------------------------- |
| 桌面 | Tauri v2                                        |
| 前端 | React 19 + Ant Design v6 + UnoCSS `presetWind4` |
| 状态 | Valtio（仅 UI 状态与设置镜像）                  |
| 后端 | Rust + sqlx + SQLite                            |
| 构建 | Vite + pnpm                                     |
| 质量 | Biome、rustfmt、clippy、cargo test              |

## 架构边界

**必须在 Rust 实现**

- 剪贴板监听、写回剪贴板、模拟粘贴，以及监听回环抑制。
- 所有数据库读写、SQLite FTS5 搜索、历史记录清理。
- 内容类型识别：URL、email、color、path。
- 窗口定位计算、OS 级键盘钩子、全局快捷键、托盘、自启。
- 图片落盘、缩略图、文件元信息读取、设置项持久化。
- Rust 侧直接展示给用户的短文案（托盘、原生右键菜单、命令返回 toast）走 `i18n/` 模块；日志与内部错误上下文不走这里。

**保留在前端**

- 组件渲染、虚拟滚动、瀑布流、动画、列表选中态。
- 主题视觉应用、CSS 变量注入、前端 i18n 文案渲染（Rust 侧文案见上）。
- HTML sanitize 与预览、RTF 渲染、Markdown 渲染。
- 普通键盘交互；Windows 主窗口收不到键时走 Rust `keyboard/` 事件。

**跨端契约**

- 前端通过 `#[tauri::command]` 调 Rust，Rust 用 `emit` 通知刷新。
- 事件名用 `domain://action`，如 `clipboard://updated`、`settings://updated`、`window://visibility`、`keyboard://nav`。
- 命令名、事件名、channel/storage key 等跨端或多处复用字面量必须集中维护：Rust 模块常量 + `src/constants/` 同步更新。

## 目录约定

```text
src-tauri/
  src/
    commands/   # tauri command 入口，只做校验与转发
    db/         # sqlx 仓储、连接池、模型
    clipboard/  # 剪贴板读写、监听、内容识别
    window/     # 窗口管理、定位、平台特化
    keystroke/  # 模拟粘贴按键注入
    keyboard/   # OS 级键盘钩子（仅 windows）
    mouse/      # 全局鼠标钩子，主窗口失焦隐藏（仅 windows）
    shortcut/   # 全局快捷键
    tray/       # 托盘菜单
    menu/       # 列表项右键菜单（macOS muda / Windows webview 窗）
    drag_out/   # OS 级拖出（文件/图片/文本拖到外部应用）
    backup/     # .ecopastebak 历史备份导出与接收
    i18n/       # Rust 侧用户可见文案（托盘、菜单、命令 toast）
    autostart/  # 开机自启
    settings/   # 设置模型与持久化
    core/       # 错误类型、路径、prevent_default（setup 在 lib.rs）
  migrations/
src/            # 前端 components/pages/stores/hooks/locales/utils
```

## 常用命令

以下是可用命令参考，按本阶段目标、实际改动和证据缺口选用；不要求每次任务全部执行，也不因列出 `-D warnings` 就把无关历史警告变为业务交付门槛。

```bash
pnpm install
pnpm tauri dev
pnpm tauri build
pnpm lint
pnpm format

cd src-tauri
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Rust 约定

- 命令与仓储函数使用 `async`，返回 `Result<T, AppError>`；`AppError` 序列化为 `{ kind, message }`。
- `message` 写用户可读根因，不加 `"xxx failed: {err}"` 动作前缀；动作上下文由前端 toast label 拼接，技术上下文写日志。
- 错误处理用 `thiserror` 定义错误类型、`anyhow` 做内部传播、`tauri-plugin-log` 记录上下文。
- 数据库使用 Tauri `State<SqlitePool>`；不要每次新建连接。
- Cargo 依赖版本不要写 patch 级完整版本；所有依赖优先写主版本号，如 `"2"`，确需收窄时最多写到 minor，如 `"0.9"`，除非有明确锁定原因。
- SQL 用 `sqlx::query` / `query_as`，不用 `query!` 宏，避免维护离线缓存。
- 已发布版本的 schema 变更必须新增 migration；已发布 migration 不回改。
- 改 schema 时同步检查所有 `SELECT`、`INSERT`、`UPDATE`、`bind`、测试结构体字面量；`query_as` 字段不匹配可能表现为 UI 空结果。
- 表必须有 `created_at` / `updated_at`，类型 `TEXT NOT NULL`；剪贴板 `updated_at` 表示内容重新使用时间，收藏、置顶、备注等元数据更新不要刷新它。
- `commands/` 保持薄层：参数校验 + 调用下层模块，不写业务逻辑。
- 平台代码用 `#[cfg(target_os = "macos")]` / `#[cfg(target_os = "windows")]` 隔离；新增能力两端同步实现，或显式标注 TODO。

## 前端约定

**React 与组件**

- 组件用 `FC<Props>`；函数体内解构 `props`，不要在参数处解构。
- 解构时需要透传剩余字段就用 `...rest` 收尾。
- React 19 优先用 Actions、`use`、`useOptimistic`、ref as prop；不要新增 `forwardRef`。
- JSX 事件回调提取为命名函数；单一动作用动词名，通用事件用 `handleXxx`。
- 箭头函数一律使用 `{}` 和显式 `return`；不要单表达式隐式返回。
- `useEffect` 只写同步副作用；异步初始化用 `useMount` + `useUnmount`，清理句柄用 `useRef`。

**状态、数据与平台 API**

- Valtio 只存 UI 状态和设置镜像；业务数据从 Rust command 拉取，不在前端建数据库副本。
- 异步统一 `async` / `await` + `try` / `catch`；不要 `.then()` / `.catch()` / `.finally()` 链式写法。
- 表达未定义用 `void 0`，不要写 `undefined`。
- 日志统一走 `@/utils/log`，禁止裸 `console.*`。
- 平台与环境判断统一从 `@/utils/is` 引入。
- 当前窗口统一用 `getCurrentWebviewWindow()`，不要用 `getCurrentWindow()`。

**样式与 UI**

- 优先使用 Ant Design v6 组件；prop 命名用 `open` / `checked` / `disabled` / `onClick`。
- 自定义 antd 内部结构优先用组件 `classNames` / `styles` 语义槽位；谨慎使用 `.ant-*` 全局覆盖。
- 样式使用 UnoCSS；条件 className 统一用 `cn from "@/utils/cn"` + 对象语法，不拼模板字符串或 `+`。
- 颜色只能用 antd token 映射类，如 `text-ant-secondary`、`bg-ant-container`、`border-ant-border`；需要新颜色先扩 `src/unocss/presetAntdColors.ts`。
- 普通文本继承全局 `text-ant-text`；次级信息优先 `text-ant-secondary`，更浅层级需有明确设计理由。
- 字号用标准语义字号：`text-xs`、`text-sm`、`text-base`、`text-lg`；不要用 `text-3` / `text-3.5`。
- 尺寸走 wind4 数字制（1 = 4px），如 `p-1.5`、`gap-2`、`rounded-2.5`、`w-36`；不要写任意 px 类或 inline px。
- 主题通过根部 `ConfigProvider` 的 `theme.algorithm` 切换，同时把 `light` / `dark` 类同步到 `<html>`。

**内容与列表**

- i18n 文案必须同步补齐 `zh-CN`（默认）和 `en-US`。
- 列表使用 `react-virtuoso` 虚拟滚动。
- HTML 内容必须经 DOMPurify sanitize 再渲染。

## 通用代码规范

- 非显然函数 / 方法上方写文档注释：TS/JS 用多行 JSDoc，Rust 用连续 `///`；getter/setter、显然一行包装、纯字面量常量可省。
- 优先早返回，避免把主流程包进嵌套 `if`。
- hooks、变量声明、副作用、不同语义阶段和 `return` 前用空行分组。
- 函数体内少写注释；只解释隐藏约束、反直觉行为或规避原因。
- 不写历史残留注释，不引用 TODO 阶段号或外部行号。
- 不做超出当前需求的抽象、兼容垫片或提前优化；React hook / 工具函数遇到真实复杂度再抽象。
- 提交信息用单行 Conventional Commits，如 `feat:`、`fix:`、`refactor:`、`docs:`。
- 改 UI 后按受影响的实际业务主路径和必要边界进行操作验证；复用仍有效的已验收证据，类型检查不能冒充真实交互通过。

## 外部文档

- Ant Design v6：<https://ant.design/components/overview-cn> · <https://ant.design/docs/react/customize-theme-cn>
- UnoCSS：<https://unocss.dev/> · <https://unocss.dev/presets/wind4>
- Tauri v2：<https://tauri.app/llms-full.txt>

<!-- TRELLIS:START -->

# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:

- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->
