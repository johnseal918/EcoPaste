# 2026-10-30 Windows 剪贴板再次不可用：8cd8b51 实机事故与 GPT 审核材料

状态：**FAILED_BUSINESS_ACCEPTANCE**。这是事故证据交接，不是修复通过报告。本次只整理并上传脱敏材料，不改业务源码、不合并 PR #3、不改基线。

## 1. 版本和构建身份

- 审核源码：[8cd8b517676b1de8ec1bdfed2e6546686158ebb6](https://github.com/johnseal918/EcoPaste/commit/8cd8b517676b1de8ec1bdfed2e6546686158ebb6)。
- 对应分支：`feature/clipboard-priority-pin-panel`；报告针对上述固定 SHA，不假定它仍是远端最新提交。
- Windows x64 Release 构建退出码 0；本地没有业务源码补丁。报告前工作区仅有未跟踪的 `.build-env/`。
- 实际交付 EXE：27,522,560 字节。
- SHA-256：`c630cc521d529933c5fffd5bfce87802af733c96ff1cdeb889299283d279f115`。10 月 30 日报告前重新计算安装位置 EXE，仍与交付记录一致。
- 安装文件名保留旧 V3 名称；不能凭文件名判定运行的是旧版。原验收已核实主进程 PID 2648、辅助进程 PID 105392 的映像与该哈希一致，主进程使用正式 prod 数据库，辅助进程未打开数据库。
- 本次进程枚举：主进程 2648，子进程先为 137704，后为 128596；父 PID 均为 2648。不能沿用“始终同一个辅助进程”的旧结论。
- `baseline/windows-clipboard-20261029` 不作任何移动、覆盖或删除。

## 2. 用户实际故障与解除占用尝试

用户报告“又无法粘贴”“先取消占用”。此前也报告系统原生粘贴失败；本次没有重新操作目标输入框，因此不得把用户反馈改写成已经自动复现的完整目标软件粘贴结果。

以下时间为北京时间；原应用日志以 UTC 记录，北京时间加 8 小时。

| 时间 | 实际证据 | 可得结论 |
| --- | --- | --- |
| 10 月 30 日 10:19:43.922 至 10:19:45.524 | 持久化的连续 5 次检测：`GetOpenClipboardWindow` 返回 HWND 2102594；`GetWindowThreadProcessId` 返回 19808；`OpenClipboard(NULL)` 全部失败，错误码 5 | 这些采样时刻，系统剪贴板确实无法打开，持锁窗口归属 PID 19808 |
| 同轮进程枚举 | PID 19808 的进程名为 `GameViewer.exe`；另有多次终端检测结果一致 | 确认当时直接持锁进程身份，不证明完整因果链 |
| 解除占用尝试 | 针对 EcoPaste 子进程 137704 的 `Stop-Process` 被 Windows 拒绝访问 | 辅助进程没有被这次外部停止尝试成功结束；不能报“已解除” |
| 用户授权后 | 仅允许暂停 UU/GameViewer 的剪贴板同步，必须保留连接；检查已有会话菜单，未成功切换任何同步开关；用户随后明确说明没有该开关并要求停止查找 | 未退出、重启或主动断开远程连接，未完成同步暂停 |
| 10:27:28.631 至 10:27:29.632 | 重新连续 5 次检测：持锁 PID 0，`OpenClipboard(NULL)` 全部成功并立即关闭 | 到此时剪贴板锁已释放；没有证据证明由哪个动作释放，也未证明目标软件粘贴已恢复 |

检测只读取锁窗口/PID并尝试打开，成功后由同一线程立即关闭；没有调用 `GetClipboardData`、`EmptyClipboard` 或写入任何剪贴板内容。没有跨线程强制 CloseClipboard。

**必须区分：应用日志里的 `owner_pid` 是内容所有者线索；独立探针的 `GetOpenClipboardWindow` 才是此处持锁窗口证据。两者不能混用。**

不能从“GameViewer 当前持锁”推导“EcoPaste 无责”；也不能从两个程序同时出现推导“EcoPaste 已被证明导致 GameViewer 卡住”。EcoPaste 是否触发了对方延迟渲染、造成/放大重试或反馈循环，仍待源码和时序审核。

## 3. 本轮发现的 EcoPaste 自身异常线索

本地应用日志快照保留了以下元数据，未上传剪贴板正文、图片、数据库或原始整份日志。

### 3.1 同一序号的短时辅助进程反复启动

UTC 02:12:20–02:12:22（北京时间 10:12:20–10:12:22），同一序号 `9327`、日志内容所有者 `Some(19808)`：

- 辅助进程启动 PID 依次包括：86640、107024、112968、83108、131988、137704（这段快照内至少 6 次）。
- 每轮可见 PNG 读取、PNG 不可读后 DIB 回退，以及 `AccessBusy / win32_code=Some(5)` 与 `FormatNotReady`。
- 所示 PNG/DIB 调用返回耗时约 0–2ms；这些日志本身并不是 8 秒超时证据。
- 不能把“新 PID 启动”直接认定为前台优先回收成功，也未在这里证明旧子进程退出、系统锁释放、写回和目标接收四个阶段全部完成。

节选：

```text
[2026-10-30][02:12:20] clipboard worker started pid=86640
[2026-10-30][02:12:20] clipboard native_read begin group=PNG_formats sequence=9327 owner_pid=Some(19808)
[2026-10-30][02:12:20] clipboard PNG unreadable; checking DIB fallback
[2026-10-30][02:12:20] clipboard capture failed category=AccessBusy win32_code=Some(5) owner_pid=Some(19808)
[2026-10-30][02:12:20] clipboard worker started pid=107024
[2026-10-30][02:12:21] clipboard worker started pid=112968
[2026-10-30][02:12:21] clipboard worker started pid=83108
[2026-10-30][02:12:22] clipboard worker started pid=131988
[2026-10-30][02:12:22] clipboard worker started pid=137704
```

### 3.2 同一序号的密集空读取

UTC 02:24:23，同一秒内快照包含 **36 条**：
`clipboard native_read begin group=text_formats sequence=9328`，对应内容所有者日志为 `None`，随后 `clipboard sequence=9328 empty_after_ms=504`。

这是同一序号密集重试的实测计数，不等于已经证明它导致 CPU 高负载或系统锁死。请审核 watcher 的重试间隔、同序号预算以及空/未就绪处理。

### 3.3 后续图片写回日志不等于接收成功

UTC 02:24:29，`paste operation=9 kind=Image`：
- write gate wait 0ms，`write_ok elapsed_ms=26`；
- `SendInput injected=4 expected=4`，`inputs_submitted`；
- 同时出现内容所有者 `Some(2648)` 的 PNG/DIB 回读和 `AccessBusy`、`FormatNotReady`，随后辅助进程 128596 启动。

上述为用户/其他自然操作期间的日志；本轮报告整理未发起该图片粘贴，也没有核验目标输入框收到图片。需审核自写回触发采集的时序，但不能凭伴随错误直接判定图片写回失败。

## 4. 已有通过证据及其限度

| 项目 | 现有证据 | 当前结论 |
| --- | --- | --- |
| Windows 构建 | 固定 SHA 编译成功，真实 EXE 已交付，哈希一致 | BUILD_PASS |
| 辅助进程正常启动/复用 | 10 月 29 日 PID 105392 在两次普通文字和两种微信复制之间复用；未观察到第二套 GUI 或数据库初始化 | 仅正常路径通过；本次已出现多次重启线索 |
| 历史文字自动粘贴 | 独立记事本真实收到测试文字并保存回读；operation=1，gate 0ms，write 1ms，SendInput 4/4 | 当时单项通过，不代表本次复发后恢复 |
| 微信聊天图片 | PNG 1919×1080、1,844,231 字节完整解码入库；native 303ms、总 330ms | 当时单项通过 |
| 微信原图窗口复制 | 同规格 PNG 完整入库；首轮序号变化被丢弃并重试成功 | 当时单项通过 |
| Excel 整列、明确复制为图片 WMF/EMF→PNG | 等待用户手动复制期间中断，没有完成验收闭环 | 未验证，不报正常 |
| 文件与 HTML/RTF | 有自然 HTML 采集元数据观察；文件/RTF 未完成定向验证 | 未完整验证 |
| 前台优先取消慢读取 | 未取得“回收辅助进程→实际释放系统锁→文字写回→目标接收”的完整实机链 | 未验证 |
| 8 秒安全上限 | 正常微信约 1.8MB 图片低于上限；未安全构造或确认真实超时路径 | 未验证大图/超时；未证实新上限造成漏采，也不能排除 |
| 父进程正常退出时辅助进程退出 | 未执行完整正常退出观察 | 未验证 |

截至报告：`CODE_COMMITTED=true`，`BUILD_PASS=true`，`REAL_WINDOWS_TEST_PASS=false`，`USER_ACCEPTED=false`。用户再次明确反馈失败，业务状态继续为 `FAILED_BUSINESS_ACCEPTANCE`。

## 5. GPT 应优先审核的问题

请以固定 SHA 的真实源码为准，检查 `src-tauri/src/clipboard/{worker.rs,read.rs,mod.rs,payload.rs,watcher.rs}` 及相关写回入口，并核实第三方 clipboard 原生调用实现：

1. AccessBusy/FormatNotReady 为什么在约两秒内引发至少六次 helper 启动？是 session 被无差别丢弃、协议错误、线程回收、watcher 重试还是其他路径？要求给出代码位置和实际分支，不能先猜。
2. 同一序号为何 504ms 内出现大量空读取？是否存在缺失退避、重试预算或因读失败触发重启的放大循环？
3. 捕获失败时是否仍请求了 PNG/DIB 延迟渲染，从而可能阻塞内容所有者或与远程剪贴板互相触发？现有证据能证明哪一段，缺哪一段？
4. 8 秒上限、前台优先信号、child kill/wait、stdout/stderr reader join 是否真正有界？helper 无窗口持锁、父进程退出、多个读取线程/等待路径是否正确收口？
5. 主进程自己的图片写回为什么紧跟 AccessBusy/FormatNotReady 和新 helper？是否需要修复具体自采集竞态，避免扩大同序号重试？
6. 如果系统锁由外部进程持有，EcoPaste 应如何停止放大读取、及时返回明确错误并保持已有历史可用？不能以退出远程软件代替 EcoPaste 自身容错。

审核先交付“已证实/推测/未知、具体文件行号、最小修复建议、Windows 定向验收步骤”。本请求授权报告和审核交接，不自动授权改代码或合并。

## 6. 保护和资料范围

- 保留原 dirty worktree、基线、正式历史数据库、收藏、置顶、备注和设置。
- 不终止或操作 UU/GameViewer、微信、Excel 或其他无关程序；用户曾对暂停同步授权例外，但没有可用开关且已要求停止查找。此后不得自行扩大到退出、重启或断开连接。
- 不清空系统剪贴板，不跨线程关闭锁，不修改 UAC、管理员权限、自启动、Office、驱动或安全配置。
- 先前验收记录显示设置及启动 action/run level 保持；应用自身启动曾刷新计划任务 Date/StartBoundary，因此不能宣称任务 XML 完全相同。本次报告未修改这些配置。
- 不重复分析已确认的 10 月 29 日 08:51 旧锁竞争，不强制制造 30 秒卡死，不新增无关测试或门槛。
- 上传内容只含源码/EXE 哈希、非敏感 PID/时序/错误码/尺寸及验收边界；不含客户正文、图片、数据库、截图、凭据或个人绝对路径。
- 本地原始日志快照仅留本机：27,782 字节，SHA-256 `f159e4cc2bb556def98cf3caeda23f5123abe65515f9f11fe8fada880f562e1b`，用于标识本次读取快照；GitHub 上的节选不等于完整原始日志。
