# QWE Technical POC Specification v0.1

这个文档的重点不是列出一些 Demo，而是建立一套可以重复执行、可以比较、可以推翻原有判断的技术验证体系。

尤其需要避免一种情况：某种语言的 POC 写得比较快，就认为它更适合整个项目。

QWE — Technical POC Specification

Rust / Go / C++ Technical Feasibility Evaluation

Status: Draft
Version: 0.1
Date: 2026-10-30

⸻

1. Objective

1.1 Purpose

本项目通过统一测试集，对 Rust、Go、C++ 三种技术路线进行工程可行性评估。

目标不是比较语言的理论性能，而是确定：

1. 哪种语言更适合实现 QWE 核心架构？
2. 哪种语言更适合实现 Terminal Runtime？
3. 哪种语言更适合实现 Process Runtime？
4. 哪种语言更容易实现稳定的 Daemon？
5. 哪种语言具有更好的系统接口和跨平台支持？
6. 哪种语言能够以较低成本支持未来扩展？

最终输出：

* 技术验证代码。
* 测试记录。
* 性能数据。
* 依赖组件评估。
* 风险清单。
* 语言选型建议。
* 架构修订建议。

1.2 Evaluation Principles

Principle 1: Same Requirements

三种语言使用相同的功能需求和验收标准。

Principle 2: Native Ecosystem Allowed

不要求三种语言使用完全相同的底层库。

允许使用各自成熟的生态。

例如：

Language	Terminal	Async / Runtime	Process
Rust	alacritty_terminal / portable-pty	Tokio	std / nix
Go	creack/pty 等	goroutine	os/exec / syscall
C++	libvterm / libuv	libuv / Boost.Asio	POSIX API

Principle 3: No Artificial Restrictions

允许使用成熟的第三方库。

不为了测试语言本身而重复实现已有基础设施。

但必须记录：

* 第三方库承担了什么职责。
* 自己实现了什么职责。
* 依赖库是否仍然需要大量适配。
* 是否存在维护和平台兼容风险。

Principle 4: Evidence-Based Evaluation

所有结论都必须有证据。

不能仅凭：

* 个人熟悉程度。
* 代码行数。
* 单次运行结果。
* 主观开发体验。
* 某个 Benchmark 的数字。

做出最终判断。

Principle 5: Architecture Validation

POC 不只是验证某个库能否运行。

还需要验证 QWE 当前的对象模型和模块边界是否合理。

⸻

2. Evaluation Scope

定义五个测试域。

ID	Domain	Weight	Priority
A	Terminal Runtime	25%	Highest
B	Process Runtime	20%	High
C	Daemon Architecture	15%	High
D	Resource Monitoring	10%	Medium
E	Developer Experience	10%	Medium

另外增加：

Evaluation	Weight
Cross-platform Capability	5%
Extension Architecture	5%
Dependency and Maintenance Cost	10%

总计 100%。

Terminal Runtime 权重最高。

这是因为 QWE 的终端能力涉及 PTY、VT、Attach/Detach、Resize、Shell Job Control 等多个不同层次，技术风险高。

⸻

3. Test Environment

3.1 Environment Matrix

第一轮建议统一在 Linux 环境完成。

推荐：

* Linux。
* x86_64。
* 至少 4 CPU Cores。
* 8 GB RAM。
* SSD。
* Bash。
* Zsh。
* Neovim。
* htop。
* SQLite。

Rust、Go、C++ 的版本和编译参数必须记录。

例如：

OS:
Kernel:
Architecture:
CPU:
Memory:
Rust:
Go:
C++ Compiler:
C++ Standard:
CMake/Meson:
Terminal:
Shell:

3.2 Secondary Environment

Linux 验证通过后，再增加：

* macOS ARM64。
* Windows（如果确定支持）。

不应在第一轮同时引入大量平台变量。

3.3 Reproducibility

每个测试需要记录：

* Test ID。
* Source Code Commit。
* Language Version。
* Dependency Version。
* Build Configuration。
* Test Command。
* Runtime Environment。
* Result。
* Logs。
* Screenshots（必要时）。
* Known Limitations。

⸻

4. Test Domain A — Terminal Runtime

4.1 Objective

验证三种语言实现 QWE Terminal Runtime 的工程可行性。

重点是：

* PTY。
* Shell。
* Terminal Emulator。
* Attach/Detach。
* Resize。
* Session Lease。
* Daemon Recovery。

需要区分：

1. PTY Management。
2. Terminal Emulator。
3. Terminal Multiplexer。
4. Session Management。

这四者不是同一个问题。

4.2 Candidate Components

Rust

优先研究：

* alacritty_terminal
* portable-pty
* Tokio

注意：

alacritty_terminal 不是完整的 Terminal Multiplexer。

需要额外实现 PTY、Session、Pane、Attach/Detach 等管理逻辑。

Go

优先研究：

* creack/pty
* os/exec
* syscall
* goroutine / channel

需要验证 VT 状态机是否采用外部组件，以及如何处理终端状态。

C++

优先研究：

* libvterm
* libuv
* POSIX PTY API
* Boost.Asio（可选）

需要评估 C++ 与系统 API 的集成成本。

4.3 Test Cases

ID	Test	Description	Acceptance
T01	PTY Create	创建和销毁 PTY	无资源泄漏
T02	Shell Launch	启动 Bash/Zsh	正常输入输出
T03	Interactive Editor	运行 Vim/Neovim	显示和交互正常
T04	Interactive Monitor	运行 htop	动态刷新正常
T05	ANSI/VT	测试控制序列	颜色、光标、清屏正常
T06	Unicode	中文、Emoji、宽字符	无明显错位
T07	Resize	调整终端尺寸	Shell 和应用正确响应
T08	Detach/Attach	断开和重新连接	状态符合预期
T09	Session Lease	测试所有权	控制权符合规则
T10	Long Output	长时间连续输出	无持续性丢失或死锁
T11	Job Control	Ctrl+C、Ctrl+Z、fg、bg	行为符合 Shell 语义
T12	Daemon Restart	Daemon 重启	验证任务与终端恢复边界

T03、T04、T05、T08、T11 必须进行人工交互验证。

4.4 Detailed Acceptance

T01 PTY Create

测试：

* 创建 PTY。
* 获取 Master/Slave。
* 启动 Shell。
* 关闭 Master。
* 回收资源。

检查：

* 文件描述符泄漏。
* 子进程退出。
* Zombie Process。
* 异常退出处理。

T03 Interactive Editor

运行 Neovim。

验证：

* 光标位置。
* 屏幕刷新。
* 全屏绘制。
* 滚动。
* 输入。
* Resize。

不能仅以普通 Shell 输出正常判断终端实现可用。

T05 ANSI/VT

覆盖：

* SGR。
* Cursor Movement。
* Erase Display。
* Erase Line。
* Alternate Screen。
* Scroll Region。
* Cursor Visibility。
* Terminal Title。

不要求第一轮自行实现 VT Parser。

重点验证选用的组件是否覆盖所需能力。

T07 Resize

测试多个尺寸变化：

120x40
80x24
160x50
40x15

检查：

* PTY Window Size。
* SIGWINCH。
* Shell 响应。
* Terminal Emulator 状态。
* Pane 尺寸更新。

T08 Detach/Attach

测试：

1. 创建 Session。
2. 启动 Shell。
3. 运行交互程序。
4. Detach。
5. 重新 Attach。
6. 验证输出和终端状态。

需要分别记录：

* PTY 是否仍然存在。
* Shell 是否仍然运行。
* 终端状态是否恢复。
* 之前的屏幕内容是否可恢复。

T09 Session Lease

验证：

* 原用户正常连接。
* 原用户断开。
* 原用户重新连接。
* 其他用户尝试接管。
* Idle Timeout。
* 程序持续输出。
* 客户端持续心跳。
* 无输入但客户端保持连接。

默认 Lease：

600 seconds

POC 可以缩短测试时间，但必须验证计时逻辑。

T12 Daemon Restart

必须区分：

Item	Expected
Task Process	尽可能继续运行
Process Identity	可以重新识别
Session Metadata	可以恢复
PTY	验证是否可恢复
Terminal Screen State	验证是否可恢复
Interactive Input	验证是否仍然可用

不允许把“任务仍然运行”描述成“终端已经恢复”。

⸻

5. Test Domain B — Process Runtime

5.1 Objective

验证任务和进程生命周期管理。

核心对象：

Task
  |
  +-- TaskInstance
        |
        +-- Process Group
              |
              +-- Process
              +-- Process

5.2 Test Cases

ID	Test	Acceptance
P01	Single Process	正常启动和退出
P02	Multi-level Process	子进程正确管理
P03	Signal Handling	SIGINT/SIGTERM 正常工作
P04	Process Group	可以统一管理进程组
P05	Child Crash	正确识别子进程异常
P06	Parent Exit	正确处理父进程退出
P07	Timeout	超时任务可以终止
P08	Daemon Crash	Task 不被无意终止
P09	PID Reuse	避免错误识别进程
P10	Restart Policy	新的执行实例有独立记录
P11	cwd/env	工作目录和环境变量正确
P12	History	TaskInstance 历史可查询

5.3 Critical Tests

P02 Multi-level Process

创建：

TaskInstance
    |
    +-- Process A
          |
          +-- Process B
                |
                +-- Process C

验证：

* 进程树识别。
* 进程组行为。
* 信号传播。
* 子进程退出。
* 任务结束判断。

P03 Signal Handling

测试：

* SIGINT。
* SIGTERM。
* SIGKILL。

注意：

SIGKILL 不可捕获。

验证 QWE 是否能够正确处理不同信号的退出语义。

P08 Daemon Crash

流程：

1. Daemon 启动 Task。
2. Task 创建多个子进程。
3. 强制终止 Daemon。
4. 检查任务进程。
5. 重启 Daemon。
6. 恢复 TaskInstance。
7. 检查状态和进程关联。

P09 PID Reuse

不能只通过 PID 判断某个进程是否仍然是原来的进程。

Linux 可以结合：

* PID。
* Process Start Time。
* Process Group。
* cgroup。
* 可用的系统进程身份信息。

验证错误识别风险。

P10 Restart Policy

分别测试：

* Never。
* OnFailure。
* Always。

每次重新执行都必须保留独立历史记录。

⸻

6. Test Domain C — Daemon Architecture

6.1 Objective

验证 QWE Daemon 的可靠性和并发处理能力。

6.2 Test Cases

ID	Test	Acceptance
D01	CLI/Daemon IPC	请求响应正确
D02	Concurrent Clients	并发请求无状态破坏
D03	Forced Daemon Exit	可重新启动
D04	SQLite Failure	错误可处理
D05	Client Disconnect	不产生残留请求
D06	Daemon Upgrade	数据和任务状态兼容
D07	Idempotency	重复请求不会产生意外副作用
D08	Long Running	长时间运行稳定

6.3 IPC

验证：

* Unix Domain Socket。
* 请求 ID。
* 请求超时。
* 客户端断开。
* 非法请求。
* 并发请求。
* 连接权限。

需要区分：

IPC 连接成功，不等于业务操作成功。

协议必须有明确的错误模型。

6.4 Idempotency

例如：

CLI 发送：

qwe task start build

由于网络或 IPC 异常，CLI 没有收到响应，再次发送相同请求。

需要验证：

是否会意外创建两个 TaskInstance。

建议支持请求 ID 或幂等键。

6.5 Persistence Failure

测试：

* SQLite 文件损坏。
* 数据库锁。
* 磁盘空间不足。
* Daemon 写入过程中退出。
* 数据库版本升级。

需要明确哪些数据可以重建，哪些数据不可丢失。

⸻

7. Test Domain D — Resource Monitoring

7.1 Objective

验证任务级资源监控能力。

重点不是开发一个完整的资源调度器，而是验证：

* 数据能否获取。
* 数据能否关联 TaskInstance。
* 采样开销是否可接受。
* 不同平台能力是否一致。

7.2 CPU

测试：

* Process CPU Usage。
* Process Group CPU Usage。
* TaskInstance CPU Usage。
* 多核情况下的 CPU 统计口径。

必须明确 CPU Usage 是：

* 单核 100% 归一化。
* 还是整机 100% 归一化。

不同系统和工具的统计口径可能不同。

7.3 Memory

测试：

* RSS。
* Process Memory。
* Process Tree Memory。
* TaskInstance Memory。

需要明确：

进程树 RSS 求和可能重复计算共享内存。

不要直接把 RSS 求和当成准确的物理内存占用。

Linux 可以进一步研究：

* PSS。
* cgroup Memory Usage。

7.4 GPU

测试环境建议包括：

* NVIDIA GPU。
* NVML。
* CUDA Runtime。
* CUDA Kernel。
* TensorRT 或 ONNX Runtime CUDA。

测试：

1. GPU Utilization。
2. GPU Memory Usage。
3. GPU Process。
4. 多进程 GPU。
5. 多 TaskInstance GPU 关联。

必须区分：

* Device Metrics。
* Process Metrics。
* Task Metrics。

不能在缺乏可靠关联依据时将 GPU 指标归因到 TaskInstance。

7.5 Monitoring Overhead

至少测试：

* 100ms 采样。
* 500ms 采样。
* 1s 采样。
* 5s 采样。

记录：

* CPU Overhead。
* Memory Overhead。
* IPC Overhead。
* Database Write Overhead。

第一阶段不要求极高频监控。

⸻

8. Test Domain E — Developer Experience

8.1 Objective

验证语言和生态是否适合持续开发 QWE。

测试：

* CLI 开发。
* TUI 开发。
* 配置解析。
* 日志。
* 错误处理。
* SQLite。
* IPC。
* Just 集成。
* 单元测试。
* 集成测试。
* 跨平台构建。

8.2 Suggested Libraries

Rust

* clap
* ratatui
* Tokio
* tracing
* serde
* rusqlite 或 sqlx
* nix
* portable-pty

Go

* cobra
* bubbletea
* zerolog 或 slog
* goroutines
* database/sql
* SQLite Driver
* PTY 库

C++

* CLI11 或 cxxopts
* FTXUI
* spdlog
* Boost.Asio 或 libuv
* SQLite
* libvterm
* POSIX API

上述组件只是候选，不代表必须全部采用。

8.3 Developer Experience Tests

至少完成：

1. 一个 CLI 命令。
2. 一个 Daemon。
3. 一个 IPC 请求。
4. 一个 TaskInstance。
5. 一个 PTY。
6. 一个 SQLite 持久化记录。
7. 一个基础 TUI 页面。

记录从空项目到可运行 Demo 的实际时间。

同时记录：

* 代码量。
* 依赖数量。
* 编译时间。
* Debug 难度。
* 测试难度。
* 错误处理复杂度。
* 第三方库适配工作量。

⸻

9. Unified Test Dataset

建议所有测试使用统一目录结构：

qwe-poc/
    README.md
    test-spec/
        terminal.md
        process.md
        daemon.md
        resource.md
        developer-experience.md
    rust/
        terminal/
        process/
        daemon/
        resource/
    go/
        terminal/
        process/
        daemon/
        resource/
    cpp/
        terminal/
        process/
        daemon/
        resource/
    test-data/
        scripts/
        shell/
        ansi/
        unicode/
        processes/
        workloads/
    results/
        rust/
        go/
        cpp/
    reports/
        comparison.md
        decision.md

9.1 Shared Test Scripts

建议将测试行为尽量抽象为外部脚本。

例如：

test-data/scripts/
    spawn_tree.sh
    cpu_stress.sh
    memory_stress.sh
    long_output.sh
    signal_test.sh
    timeout_test.sh
    child_crash.sh
    daemon_crash.sh

三种语言调用相同的测试脚本。

避免因为测试负载不同造成比较偏差。

⸻

10. Measurement and Scoring

10.1 Score Scale

统一使用 1–5 分。

Score	Meaning
1	无法满足需求
2	可以实现，但存在明显缺陷
3	满足基本要求
4	满足要求，工程实现较成熟
5	满足要求，具有明显工程优势

每个评分必须附带测试证据。

10.2 Weighted Score

Total Score =
    Terminal Runtime       * 25%
  + Process Runtime        * 20%
  + Daemon Architecture    * 15%
  + Resource Monitoring    * 10%
  + Developer Experience   * 10%
  + Cross-platform         * 5%
  + Extension Architecture * 5%
  + Maintenance Cost       * 10%

每个领域的分数由多个测试项综合得出。

不能简单使用某个单项 Benchmark 替代整体评分。

10.3 Additional Metrics

除评分外，记录：

Metric	Description
Implementation Time	实际实现时间
LOC	代码量，仅供参考
Dependency Count	主要依赖数量
Build Time	编译时间
Runtime Memory	Daemon 内存
Runtime CPU	Daemon CPU
Startup Latency	Daemon 启动时间
IPC Latency	IPC 延迟
Recovery Time	恢复时间
Failure Rate	测试失败率
Maintenance Complexity	维护复杂度

性能测试至少重复多次。

建议报告：

* Median。
* P95。
* Maximum。
* Standard Deviation（必要时）。

⸻

11. POC Execution Plan

Stage 1 — Basic Runtime

目标：

先验证三种语言的系统编程能力。

完成：

* PTY。
* Shell。
* Process Group。
* Signal。
* IPC。
* SQLite。

Stage 2 — Terminal Feasibility

完成：

* Terminal Emulator Integration。
* Resize。
* Unicode。
* ANSI/VT。
* Detach/Attach。
* Session Lease。

这是最重要的阶段。

Stage 3 — Reliability

完成：

* Daemon Crash。
* Process Recovery。
* TaskInstance Recovery。
* PID Reuse。
* Persistence Failure。

Stage 4 — Resource Monitoring

完成：

* CPU。
* Memory。
* Process Tree。
* GPU 基础监控。

Stage 5 — Comparative Evaluation

完成：

* 评分。
* 依赖评估。
* 技术风险分析。
* 最终语言选择。

⸻

12. Decision Criteria

最终选型不能只看总分。

建议设置以下硬性门槛：

1. Terminal Runtime 必须满足基本交互要求。
2. Process Runtime 必须可靠管理进程组。
3. Daemon Crash 后不能意外终止后台任务。
4. TaskInstance 必须具备可恢复的身份识别方式。
5. 关键依赖必须具备可接受的维护状态。
6. IPC 和持久化必须具有明确错误处理机制。

如果某种语言总分较高，但 Terminal Runtime 或 Daemon Recovery 无法满足要求，不应选择该语言。

12.1 Decision Output

最终报告至少包括：

1. Evaluation Summary
2. Test Environment
3. Test Results
4. Performance Comparison
5. Dependency Assessment
6. Engineering Complexity
7. Architecture Risks
8. Language Comparison
9. Recommended Language
10. Reasons for Rejection
11. Remaining Unknowns

12.2 Final Decision Principle

POC 的目标不是证明某种语言最好。

而是发现：

* 哪些需求容易实现。
* 哪些需求需要复杂适配。
* 哪些第三方组件不适合。
* 哪些架构假设不成立。
* 哪些技术风险会在正式开发时产生高昂成本。

最终选择应同时考虑当前开发成本和未来维护成本。

POC 的价值在于尽早暴露错误的技术选择，而不是尽早确定一个看起来正确的答案。


## 13. 交付后的执行建议

三个文档形成了一个基本的设计闭环，但还不建议马上进入完整开发。

我建议按这个顺序执行：

顺序	工作	产出	目的
1	审查 Design Baseline	v0.2 冻结版	确认对象模型和架构边界
2	建立 POC 工程骨架	Rust/Go/C++ 三个工程	保证测试环境一致
3	优先执行 Terminal POC	Terminal 测试报告	解决最大技术风险
4	执行 Process/Daemon POC	进程与恢复测试报告	验证核心可靠性
5	综合评分	Language Decision Report	决定主要技术路线
6	更新 Baseline	v0.4	将 POC 与恢复/并发结论纳入正式架构

有一个需要特别强调的判断：不要先做 CLI、TUI 或者漂亮的分屏界面。

