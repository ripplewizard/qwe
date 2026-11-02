# QWE Design Baseline v0.2

这是 v0.1 的完整修订版，保留原文档 ID。

QWE — Quick Window Environment

Project Design Baseline v0.2

Status: Design Baseline
Version: 0.2
Date: 2026-10-30

⸻

1. Project Overview

1.1 Project Definition

QWE（Quick Window Environment）是一个轻量级、可扩展的开发者任务运行环境。

它为开发者提供统一的终端管理、任务执行、进程生命周期管理和资源观测能力。

QWE 不只是一个 Terminal Multiplexer，也不是一个完整的容器管理平台或分布式任务调度系统。

它的核心目标是：

* 统一管理开发者的终端工作环境。
* 统一管理交互式和非交互式任务。
* 使任务能够脱离客户端连接持续运行。
* 提供任务生命周期管理和基础资源观测。
* 支持通过配置和命令组织复杂开发工作流。
* 为未来的多机执行、Agent Runtime 和安全隔离提供扩展基础。

1.2 Design Philosophy

QWE 遵循以下设计原则。

P01. Lightweight Implementation

当前版本优先实现核心能力。

不因为未来可能需要某项功能，就在当前引入复杂的依赖、服务或抽象层。

P02. Extension-Aware Architecture

架构设计需要考虑明确可预见的扩展方向。

扩展功能可以延后实现，但应提前研究：

* 对象模型是否需要变化。
* 模块边界是否需要调整。
* 当前接口是否会形成技术限制。
* 是否存在需要提前保留的扩展点。

避免未来功能加入时大规模重构。

P03. Explicit Boundaries

模块职责、对象生命周期、数据所有权和依赖关系必须明确。

避免一个模块同时承担终端管理、进程管理、资源控制和业务编排。

P04. Runtime Independence

QWE 的核心模型不应绑定某一种底层执行运行时。

例如：

* PTY 可以有不同实现。
* Process Runtime 可以有不同实现。
* Resource Controller 可以有不同实现。
* Environment Backend 可以有不同实现。

具体实现由平台能力和技术验证结果决定。

P05. Capability-Oriented Design

不同操作系统和不同运行时提供的能力并不一致。

QWE 应显式描述能力，而不是假设所有平台都支持相同功能。

例如：

* CPU 使用率监控。
* 内存限制。
* GPU 监控。
* GPU 资源隔离。
* 进程树管理。
* Sandbox。

能力不可用时，应明确返回 Unsupported 或 Unavailable，而不是伪装成已经实现。

P06. Observability First

任务执行必须具备基本可观测性。

至少应能够回答：

* 任务是否正在运行？
* 任务由哪些进程组成？
* 任务什么时候启动？
* 任务什么时候结束？
* 任务为什么退出？
* CPU 和内存使用情况如何？
* 任务是否发生异常？
* Daemon 重启后，任务是否仍然存在？

P07. Reliability First

不能为了实现速度牺牲任务可靠性。

尤其需要保证：

* 客户端退出不意外终止任务。
* Daemon 崩溃不主动杀死仍在运行的任务。
* Daemon 重启后能够重新识别任务。
* 任务状态不能只依赖内存状态。
* 进程标识不能只依赖 PID。

⸻

2. Product Scope

2.1 Current Development Scope

当前版本优先实现以下模块。

Module	Description	Priority
Terminal Management	Session、Window、Pane、PTY 管理	P0
Task Runtime	任务创建、执行、停止、状态管理	P0
Process Management	Process Group、进程树和生命周期管理	P0
Session Ownership	Session 控制权和 Lease	P0
Basic Monitoring	CPU、内存、进程状态	P1
Persistence	SQLite 元数据和状态恢复	P0
CLI	统一命令行接口	P0
Just Integration	与 Justfile 集成	P1
Environment Definition	环境配置和恢复描述	P1

P0 表示核心系统必须具备的能力。

P1 表示第一阶段核心功能稳定后实现。

2.2 Explicitly Out of Current Scope

以下功能不属于当前版本的直接开发目标。

Feature	Current Decision
Multi-node Execution	独立设计，当前不实现
QWE Mesh	独立设计，当前不实现
Distributed Scheduler	暂不实现
Security Sandbox	预留 Resource/Runtime 扩展边界
Credential Broker	独立研究，暂不实现
LLM Proxy	优先评估成熟方案
Agent Runtime	作为未来扩展方向
GPU Resource Isolation	当前只考虑监控和能力描述
Full Environment Reproduction	当前只保存环境定义和关键元数据

不在当前开发范围，不代表架构上完全忽略。

具体扩展要求见：

QWE Extension Architecture Notes

⸻

3. System Architecture

3.1 High-Level Architecture

QWE 采用 Client-Server 架构。

主要组成：

1. QWE CLI
2. QWE Daemon
3. Terminal Runtime
4. Task Runtime
5. Process Manager
6. Resource Monitor
7. Persistence Layer
8. Extension Interfaces

逻辑结构：

                  QWE CLI
                     |
                     | IPC
                     |
              +-------------+
              | QWE Daemon  |
              +-------------+
                     |
       +-------------+-------------+
       |             |             |
       v             v             v
 Terminal       Task Runtime    Persistence
 Runtime             |             |
       |             v             |
       |        Process Manager    |
       |             |             |
       v             v             |
    PTY / TTY      OS Process      |
       |             |             |
       +-------------+-------------+
                     |
                     v
               Resource Monitor
                     |
                     v
                Metrics Store

该结构表达逻辑职责，不要求当前版本必须拆分成多个独立进程或微服务。

第一阶段优先采用单 Daemon、多模块的实现方式。

3.2 CLI

CLI 是用户操作 QWE 的主要入口。

负责：

* 创建和管理 Session。
* 创建和管理 Window、Pane。
* 创建和管理 Task。
* 连接和断开 Session。
* 查看任务状态。
* 查询资源使用情况。
* 管理配置。
* 调用 Justfile 任务。

CLI 不直接负责：

* 任务生命周期管理。
* PTY 长期持有。
* 资源采样。
* 任务状态持久化。

上述职责由 Daemon 完成。

3.3 Daemon

Daemon 是 QWE 的核心控制进程。

负责：

* 管理运行时对象。
* 管理 Terminal Runtime。
* 管理 Task Runtime。
* 管理 Process 生命周期。
* 管理 Session Lease。
* 管理任务状态。
* 处理客户端 IPC。
* 记录持久化状态。
* 提供资源观测接口。
* 在重启后恢复运行时状态。

Daemon 应尽可能避免成为任务生命周期的唯一依赖。

Task 应能够在 Daemon 退出后继续运行。

Daemon 恢复时，应通过系统进程信息重新识别 TaskInstance。

3.4 Persistence Layer

第一阶段使用 SQLite 存储：

* Session 元数据。
* Task 定义。
* TaskInstance 历史记录。
* Task 状态变化。
* Workspace Definition。
* 配置。
* 关键事件。

不建议将高频终端输出和全部实时指标直接写入 SQLite。

终端输出和日志应使用独立的文件或流存储机制。

实时指标应考虑采样频率、保留时间和数据库膨胀。

⸻

4. Core Object Model

QWE 需要明确区分 Terminal Model、Execution Model 和 Configuration Model。

这是当前架构最重要的边界之一。

4.1 Terminal Model

Terminal Model 负责描述用户看到和操作的终端结构。

Session
  |
  +-- Window
  |     |
  |     +-- Pane
  |     |
  |     +-- Pane
  |
  +-- Window
        |
        +-- Pane

Session

Session 是一组持续存在的终端工作环境。

职责：

* 管理 Window。
* 管理当前控制权。
* 管理客户端连接状态。
* 关联可选的 TaskInstance。
* 支持 Detach 和 Attach。

Session 不直接等同于一个 Task。

Session 关闭也不应默认终止所有与其关联的后台任务。

Window

Window 是 Session 内部的逻辑终端窗口。

负责组织 Pane。

Pane

Pane 是一个终端显示区域。

负责：

* 关联 PTY。
* 管理终端尺寸。
* 接收和转发输入。
* 展示终端输出。
* 可选关联 TaskInstance。

Pane 不拥有 TaskInstance 的生命周期。

PTY

PTY 是终端交互的底层机制。

PTY 管理需要处理：

* Master/Slave。
* Shell 启动。
* 输入输出。
* Terminal Resize。
* 进程退出。
* 文件描述符生命周期。

QWE 不自行实现完整 VT Parser。

优先使用成熟的终端模拟器组件。

需要注意：

Terminal Emulator 和 Terminal Multiplexer 是两个不同概念。

终端模拟器组件可以负责 VT 状态机，但不会自动提供完整的 Session、Pane、Attach/Detach 和多窗口管理。

⸻

4.2 Execution Model

Execution Model 负责描述任务及其执行过程。

Task
 |
 +-- TaskInstance
       |
       +-- Process
       |
       +-- Process
       |
       +-- Process

Task

Task 是任务定义。

例如：

* 编译项目。
* 运行测试。
* 启动开发服务器。
* 执行模型推理。
* 运行训练任务。

Task 可以描述：

* Command。
* Arguments。
* Working Directory。
* Environment。
* Timeout。
* Restart Policy。
* Resource Policy。

Task 本身不代表一个正在运行的进程。

TaskInstance

TaskInstance 是一次具体的任务执行实例。

例如：

同一个 Task 执行三次，会产生三个 TaskInstance。

TaskInstance 是：

* 生命周期管理单位。
* 状态记录单位。
* 资源监控关联单位。
* 执行历史记录单位。
* Resource Policy 的逻辑应用单位。

Process

Process 是操作系统中的实际进程。

一个 TaskInstance 可以包含多个进程。

Process Group

Process Group 是 Linux/Unix 下重要的进程管理单位。

QWE 应尽可能将一个 TaskInstance 的相关进程组织在独立的 Process Group 中。

用于：

* 统一发送信号。
* 管理子进程。
* 终止任务。
* 管理 Shell Job Control。

Process Group 不等同于 TaskInstance。

TaskInstance 是 QWE 的逻辑对象，Process Group 是操作系统提供的执行组织机制。

未来可以通过 cgroup 等机制提供更完整的进程树管理。

⸻

4.3 Terminal Model and Execution Model Relationship

Terminal Model 与 Execution Model 必须保持解耦。

两者之间允许建立可选关联：

Session
  |
  +-- Window
        |
        +-- Pane
              |
              +-- optional TaskInstance reference
Task
  |
  +-- TaskInstance
        |
        +-- Process Group

典型场景：

1. 用户在 Pane 中启动一个交互式 Shell。
2. 用户在 Pane 中启动一个开发服务器。
3. 用户从 CLI 启动后台 Task。
4. 后台 Task 没有对应 Pane。
5. 用户从不同 Pane 查看同一个 Task 的日志或状态。

不能因为某个 TaskInstance 关联了 Pane，就让 Pane 成为 TaskInstance 的生命周期所有者。

⸻

4.4 Configuration Model

Workspace 是配置定义，不是运行时对象。

Workspace Definition

Workspace 用于描述一个项目或工作环境。

可以包含：

* Project Root。
* Default Working Directory。
* Environment Variables。
* Justfile Location。
* Default Task Definitions。
* Terminal Layout。
* Environment Metadata。

Workspace 不参与运行时生命周期状态机。

Workspace 不拥有 Session。

Workspace 不拥有 TaskInstance。

Workspace 的职责是为 Session 和 Task 提供配置上下文。

Workspace Definition
       |
       +-- Session Configuration
       |
       +-- Task Definitions
       |
       +-- Environment Metadata
       |
       +-- Justfile Integration

未来可以扩展 Workspace Backend，但不应把 Workspace 设计成一个必须长期运行的服务。

⸻

5. Session Ownership and Lease

5.1 Design Goal

QWE 当前不考虑多用户协作控制。

一个 Session 同一时间只允许一个控制客户端。

当用户进入 Session 后，Session 的控制权被锁定。

客户端退出或断开连接后，控制权不会立即释放。

同一用户可以重新进入。

其他用户需要等待 Lease 超时后才能接管。

5.2 Ownership Model

Session Lease 至少包含：

SessionLease
{
    owner_identity
    acquired_at
    last_input_at
    idle_timeout
    connected
}

第一阶段的用户身份使用操作系统用户身份。

例如 Linux UID。

未来跨机器时，再考虑 QWE Identity。

5.3 Idle Timeout

默认：

idle_timeout = 600 seconds

即 10 分钟。

支持配置。

Lease 是否过期，根据：

current_time - last_input_at >= idle_timeout

判断。

重要：

Idle Timeout 从最后一次真实用户输入开始计算，而不是从客户端断开时间开始计算。

因此，即使客户端仍然连接，只要持续 10 分钟没有真实输入，也可以释放控制权。

5.4 What Counts as User Input

以下行为刷新 last_input_at：

* 键盘输入。
* 鼠标操作。
* 发送给 PTY 的有效终端输入。
* 其他明确代表用户主动控制的输入。

以下行为不刷新：

* 程序输出。
* Daemon 心跳。
* 客户端心跳。
* Resize。
* 后台任务执行。
* 自动刷新状态。

不能把程序输出视为用户活动。

5.5 Ownership State

建议状态：

UNOWNED
OWNED_CONNECTED
OWNED_DISCONNECTED
LEASE_EXPIRED

状态转换：

UNOWNED
   |
   | acquire
   v
OWNED_CONNECTED
   |
   | disconnect
   v
OWNED_DISCONNECTED
   |
   | same owner reconnect
   v
OWNED_CONNECTED

无论客户端是否连接：

last_input_at + idle_timeout
             |
             v
        LEASE_EXPIRED
             |
             v
          UNOWNED

其他用户在 Lease 未过期时不能获得控制权。

Lease 过期后，允许新的控制客户端获取控制权。

5.6 Control Client Policy

第一阶段：

* 一个 Session 只允许一个控制客户端。
* 其他客户端不能同时写入 PTY。
* 可以在后续版本支持只读观察客户端。
* 同一 OS 用户是否允许多个客户端同时连接，作为后续决策项。

Lease 只管理 Session 控制权，不影响 TaskInstance 的生命周期。

⸻

6. Task Runtime

6.1 Task Lifecycle

TaskInstance 建议采用以下状态：

CREATED
   |
   v
STARTING
   |
   v
RUNNING
   |
   +--------> STOPPING
   |             |
   |             v
   |          STOPPED
   |
   +--------> EXITED
   |
   +--------> FAILED

必要时增加：

* TIMEOUT
* LOST
* RECOVERING

状态定义必须区分：

* 正常退出。
* 主动停止。
* 异常退出。
* 超时退出。
* Daemon 暂时无法确认状态。

不能简单地把 Daemon 重启后的任务全部标记为失败。

6.2 Process Management

第一阶段优先支持：

* 创建进程。
* 获取进程标识。
* Process Group。
* SIGINT。
* SIGTERM。
* SIGKILL。
* 等待进程退出。
* 获取退出码。
* 管理 Working Directory。
* 管理 Environment Variables。

需要考虑：

* 多级子进程。
* 子进程异常退出。
* 父进程退出。
* PID Reuse。
* Zombie Process。
* Daemon Crash。

6.3 Task Restart

Restart Policy 作为 Task 定义的一部分。

建议第一阶段支持：

* Never
* OnFailure
* Always

但需要明确：

TaskInstance 是一次执行。

Restart 应创建新的 TaskInstance，或者通过明确的 Restart Attempt 记录执行历史。

不能覆盖旧的执行记录。

⸻

7. Resource Management and Monitoring

7.1 Resource Model

资源策略挂载在 TaskInstance 上。

不是 Pane。

不是 Workspace。

Task
 |
 +-- TaskInstance
       |
       +-- Resource Policy
       |
       +-- Process Group
       |
       +-- Metrics

7.2 First-Stage Metrics

第一阶段优先支持：

Resource	Metrics
CPU	CPU Usage
Memory	RSS
Process	PID、Process Count
Lifecycle	Start Time、Exit Time、Exit Code
IO	Basic IO Statistics, if available

GPU 监控可以作为后续 P1/P2 功能。

7.3 Resource Controller

需要区分：

Resource Monitoring 和 Resource Control。

Monitoring 用于观察资源使用情况。

Control 用于限制资源使用。

第一阶段优先实现 Monitoring。

Linux 环境未来可以通过 cgroups v2 实现 CPU、Memory、IO 等限制。

TaskInstance 是逻辑管理对象。

cgroup 是实际操作系统资源控制机制。

两者不能视为同一个对象。

7.4 GPU

GPU 资源能力需要单独建模。

注意：

CUDA_VISIBLE_DEVICES 只控制设备可见性，不提供显存限制。

Linux cgroups v2 也不提供通用 GPU 显存限制能力。

第一阶段可以使用 NVML 等接口获取：

* GPU Utilization。
* GPU Memory Usage。
* GPU Process Information。

但必须区分：

* GPU 设备级数据。
* GPU 进程级数据。
* TaskInstance 级聚合数据。

不能在没有可靠进程关联信息的情况下，将设备级 GPU 使用率直接归因到某个 TaskInstance。

⸻

8. Terminal Runtime

8.1 Requirements

Terminal Runtime 是 QWE 的核心技术风险之一。

至少需要支持：

* PTY 创建和销毁。
* Shell 启动。
* 输入输出转发。
* Detach/Attach。
* Resize。
* ANSI/VT 控制序列。
* Unicode 和宽字符。
* Shell Job Control。
* 长时间运行。
* Daemon Restart Recovery。

8.2 Terminal Emulator

QWE 不自行实现完整 VT Parser。

优先研究成熟组件：

* Rust: alacritty_terminal
* C/C++: libvterm
* 其他可行的终端模拟器组件

这些组件解决的是终端模拟器相关问题，不等同于完整 Multiplexer。

QWE 仍然需要处理：

* Session。
* Window。
* Pane。
* PTY。
* Attach/Detach。
* Session Lease。
* 输入输出路由。
* Resize。
* Daemon 生命周期。

8.3 Terminal Runtime Architecture

建议将以下职责分开：

Terminal Management
        |
        +-- Session Manager
        |
        +-- Window Manager
        |
        +-- Pane Manager
        |
        +-- PTY Backend
        |
        +-- Terminal Emulator Backend
        |
        +-- Input/Output Router

具体实现由 POC 结果决定。

⸻

9. IPC and Daemon Reliability

9.1 IPC

CLI 与 Daemon 通过本地 IPC 通信。

第一阶段优先考虑：

* Unix Domain Socket。
* 明确的消息协议。
* 请求 ID。
* 错误码。
* 超时。
* 客户端断开处理。

不应让 CLI 直接修改 Daemon 的内部状态。

9.2 Daemon Crash Recovery

Daemon 崩溃恢复是核心要求。

必须区分：

1. Daemon 进程退出。
2. Task 进程退出。
3. PTY 文件描述符失效。
4. Session 元数据仍然存在。
5. TaskInstance 仍然存在。

Daemon 重启后需要：

* 读取持久化记录。
* 识别仍然存在的 TaskInstance。
* 重新关联进程。
* 判断任务是否仍然运行。
* 更新状态。
* 标记无法确认的任务。
* 恢复可恢复的 Session 元数据。

不能只使用 PID 识别进程。

需要考虑 PID Reuse。

Linux 上可以结合：

* PID。
* /proc/<pid>/stat 中的启动时间信息。
* Process Group。
* cgroup。
* 其他可用的进程身份信息。

实际恢复策略需要通过 POC 验证。

9.3 Important Limitation

Daemon 崩溃后，PTY Master 文件描述符可能无法恢复。

因此：

任务进程继续运行，不代表原终端交互会话一定可以恢复。

必须分别验证：

* Process Survival。
* Task Recovery。
* Terminal Recovery。

不能将三者混为一谈。

⸻

10. Justfile Integration

Just 是任务命令组织工具。

QWE 负责任务运行上下文和生命周期管理。

两者职责不同。

典型流程：

Workspace Definition
       |
       v
    Justfile
       |
       v
    QWE CLI
       |
       v
  Task Runtime
       |
       v
   TaskInstance

QWE 应尽可能复用 Just 已有的任务定义能力。

不重复实现一套完整的任务依赖和命令组织系统。

第一阶段重点：

* 读取或调用 Justfile。
* 将 Just Recipe 映射为 Task。
* 支持任务执行。
* 记录 TaskInstance。
* 提供执行状态和日志查询。

⸻

11. Security Baseline

第一阶段重点关注本地权限边界。

至少包括：

* Daemon Socket 权限。
* IPC 客户端身份验证。
* 本地文件权限。
* 配置文件权限。
* 进程启动参数处理。
* 日志中的 Secret 脱敏。

11.1 Credentials

不允许将真实 API Key 直接写入：

* 命令行参数。
* 普通日志。
* 明文持久化配置。

环境变量也不能视为安全隔离机制。

第一阶段不自行开发完整 Vault。

优先评估成熟凭据管理方案。

未来 Credential Broker 的设计见 Extension Architecture Notes。

11.2 Security Scope

第一阶段不承诺：

* 多用户安全隔离。
* 完整 Sandbox。
* 对抗同 UID 恶意进程。
* 分布式身份认证。
* 多机安全授权。

这些能力需要独立设计。

⸻

12. Configuration and Persistence

配置应区分：

1. Global Configuration
2. Workspace Definition
3. Session Configuration
4. Task Definition
5. Runtime State

建议目录结构：

~/.config/qwe/
    config.toml
    workspaces/
        project-a.toml
        project-b.toml
~/.local/share/qwe/
    qwe.db
    sessions/
    logs/
    tasks/

实际目录结构可以根据平台规范调整。

持久化原则：

* 配置和运行时状态分离。
* 任务定义和任务实例分离。
* 日志与结构化元数据分离。
* 可恢复状态必须有明确的恢复策略。
* 不把内存对象序列化当作唯一恢复方案。

⸻

13. Extension Architecture

QWE 采用面向扩展的架构，但当前版本不实现全部扩展功能。

需要提前研究以下方向：

ID	Extension	Current Status
E01	QWE Mesh	Architecture Reserved
E02	Resource Controller	Monitoring First
E03	Credential Broker	Research Only
E04	Security Sandbox	Research Only
E05	Agent Runtime	Research Only
E06	Environment Backend	Basic Definition
E07	Terminal Backend	POC Required

扩展设计必须明确：

* 对核心对象模型的影响。
* 模块依赖方向。
* 需要预留的接口。
* 当前不实现的原因。
* 未来实现触发条件。

不要为不确定的未来需求引入过度抽象。

⸻

14. Technical Decisions

ID	Decision	Status
D001	QWE uses Extension-Aware Architecture	Confirmed
D002	Extension boundaries are researched before implementation	Confirmed
D003	Workspace is a configuration definition, not a runtime object	Confirmed
D004	Terminal Model and Execution Model are independent	Confirmed
D005	TaskInstance is the logical resource management unit	Confirmed
D006	Pane and TaskInstance have an optional association	Confirmed
D007	One Session allows one control client at a time	Confirmed
D008	Session uses Owner + Idle Timeout Lease	Confirmed
D009	Default Lease timeout is 10 minutes	Confirmed
D010	Program output and heartbeat do not refresh Lease	Confirmed
D011	Reuse a mature VT/Terminal Emulator component	Principle Confirmed
D012	Rust, Go and C++ use a unified POC test suite	Confirmed
D013	Terminal Runtime is the highest-priority language evaluation item	Confirmed
D014	Daemon Crash Recovery is a core requirement	Confirmed
D015	QWE Mesh is independently designed, with extension space reserved	Confirmed

14.1 Pending Decisions

以下事项仍需通过 POC 或进一步设计确定：

ID	Question
PD001	Rust、Go、C++ 最终采用哪一种语言？
PD002	Terminal Emulator Backend 最终采用哪个组件？
PD003	第一阶段是否允许同一 OS 用户多个客户端连接同一 Session？
PD004	是否支持只读观察客户端？
PD005	Daemon Crash 后 Terminal Recovery 的具体策略？
PD006	TaskInstance 的进程身份识别方案？
PD007	第一阶段支持哪些操作系统？
PD008	是否需要 GPU Process Monitoring 纳入第一阶段？

⸻

15. Development Strategy

建议按以下阶段推进。

Phase 0 — Technical Feasibility

目标：

* Rust/Go/C++ POC。
* Terminal Runtime 验证。
* Process Runtime 验证。
* Daemon Recovery 验证。
* 选择主要技术路线。

Phase 1 — Core Runtime

实现：

* Daemon。
* CLI。
* IPC。
* Task Runtime。
* Process Management。
* Persistence。
* Basic Session Management。

Phase 2 — Terminal Runtime

实现：

* Session。
* Window。
* Pane。
* PTY。
* Attach/Detach。
* Resize。
* Session Lease。

Phase 3 — Observability

实现：

* CPU。
* Memory。
* Process Tree。
* Task Metrics。
* 日志与历史记录。

Phase 4 — Developer Workflow

实现：

* Justfile Integration。
* Workspace Definition。
* Environment Metadata。
* CLI/TUI 改进。

Phase 5 — Extension Development

根据实际需求选择：

* Resource Controller。
* Credential Broker。
* QWE Mesh。
* Security Sandbox。
* Agent Runtime。

⸻

16. Definition of Done

QWE 核心版本至少需要满足：

1. 能够创建和管理 Session。
2. 能够创建 Window 和 Pane。
3. 能够运行交互式 Shell。
4. 支持 Detach/Attach。
5. 支持终端 Resize。
6. 能够创建和管理 Task。
7. Task 能够脱离 CLI 生命周期继续运行。
8. 能够管理 Process Group。
9. 能够正确处理 SIGINT、SIGTERM 和 SIGKILL。
10. 能够记录 TaskInstance 历史。
11. Daemon 重启后能够重新识别运行中的 Task。
12. Session Lease 能够正确处理控制权。
13. 能够查看基本 CPU 和内存指标。
14. CLI 与 Daemon 通过明确的 IPC 协议通信。
15. Justfile 可以触发 QWE Task。
16. 关键错误和状态变化有可追踪记录。

最终目标不是复制 tmux，也不是构建一个缩小版 Kubernetes。

QWE 的核心价值在于：

将终端、任务、进程和资源观测统一到一个轻量、可持续运行、可扩展的开发者工作环境中。

