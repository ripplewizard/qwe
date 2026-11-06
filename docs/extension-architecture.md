# QWE Extension Architecture

这份文档与前两个文档不同：它不是当前开发任务列表，而是未来架构变化的研究记录。

这里尤其强调一个原则：预留扩展能力，不等于提前实现扩展框架。

QWE — Extension Architecture Notes

Future Capabilities and Architecture Extension Boundaries

Status: Research Notes
Updated: 2026-11-04

⸻

1. Purpose

本文档用于研究 QWE 未来可能需要的架构扩展。

当前版本不要求实现这些功能。

主要目标：

1. 识别可能影响核心对象模型的未来需求。
2. 识别当前架构中需要预留的扩展边界。
3. 避免未来新增功能时大规模重构。
4. 避免为了不确定的需求提前引入复杂抽象。
5. 明确每项扩展的技术风险和实现触发条件。

本文档不是未来功能的承诺，也不是开发路线图。

它是一份架构风险和扩展方向登记文档。

⸻

2. Extension Design Principles

2.1 Separate Extension Research from Implementation

扩展方向可以提前研究。

但只有在实际需求出现时，才进入正式实现。

例如：

* 可以提前研究多机执行。
* 当前不实现分布式调度器。
* 可以提前研究 Credential Broker。
* 当前不实现完整 Vault。
* 可以提前研究 Sandbox。
* 当前不实现容器运行时。

2.2 Preserve Stable Core Models

当前核心对象：

Workspace Definition
       |
       v
Terminal Model
Task
 |
 +-- TaskInstance
       |
       +-- Process Group

未来扩展应尽量在不改变这些核心对象基本语义的情况下实现。

如果扩展确实需要修改核心模型，应明确记录变更原因。

2.3 Prefer Capability Interfaces

不同平台、不同运行时的能力可能不一致。

例如：

* Linux 可以通过 cgroups v2 实现部分资源控制。
* macOS 使用不同的资源管理机制。
* Windows 有 Job Objects。
* GPU 资源控制能力依赖具体驱动和平台。

因此不应将 Linux 特有机制直接暴露为 QWE 的通用业务模型。

应先定义抽象能力，再由平台 Backend 实现。

2.4 Avoid Premature Generalization

不建议第一阶段建立一个通用插件系统来承载所有未来功能。

也不建议为了未来多机调度，提前将每个函数都抽象成 RPC。

当前优先：

* 明确接口边界。
* 明确对象归属。
* 明确生命周期。
* 明确扩展点。
* 保留必要的标识和元数据。

⸻

3. Extension Registry

ID	Extension	Priority	Current Status
E01	QWE Mesh	High	Research
E02	Resource Controller	High	V0 cgroup v2 controls implemented; Stage 7 hardening
E03	Credential Broker	High	Research
E04	Security Sandbox	Medium	Research
E05	Agent Runtime	Medium	Research
E06	Environment Backend	Medium	Basic Definition
E07	Terminal Backend	High	POC Required

⸻

4. E01 — QWE Mesh

4.1 Objective

QWE Mesh 用于支持多台机器之间的任务执行和状态管理。

典型场景：

* 在本机开发。
* 在远程 GPU 服务器执行推理。
* 在另一台机器执行训练。
* 在多台机器之间查看任务状态。
* 从本机管理远程任务。

4.2 Potential Architecture

未来可以采用：

                QWE CLI
                   |
                   v
             Local Daemon
                   |
              Mesh Control
                   |
       +-----------+-----------+
       |           |           |
       v           v           v
    Node A      Node B      Node C
       |           |           |
    Runtime     Runtime     Runtime
       |           |           |
    Process     Process     Process

每台机器运行一个 QWE Node。

Local Daemon 负责本地控制和用户交互。

远程 Node 负责本地任务执行。

4.3 Important Object Model

当前 TaskInstance 的定义不应直接假设执行位置永远是本机。

未来可以引入：

ExecutionTarget
{
    target_type
    node_id
    platform
    capabilities
}

当前可以默认：

ExecutionTarget = Local

未来再支持：

ExecutionTarget = Remote

但第一阶段不需要实现完整的 Remote Execution。

4.4 Potential Communication

候选：

* gRPC。
* HTTP/2。
* SSH。
* 自定义 RPC。

需要考虑：

* 身份认证。
* 双向认证。
* 授权。
* 网络断开。
* 重试。
* 请求幂等性。
* 任务状态同步。
* 日志传输。
* 版本兼容。

mTLS 只解决传输层身份和加密问题，不等于应用层授权。

4.5 Security

至少需要：

* Node Identity。
* Mutual Authentication。
* Application Authorization。
* Task Permission。
* Node Registration。
* Revocation。
* Audit Log。

4.6 Current Architecture Reservation

当前建议：

1. TaskInstance 保留执行目标概念的扩展空间。
2. Process Runtime 不直接依赖 CLI。
3. IPC 消息协议避免将本地路径和本地 PID 视为全局身份。
4. TaskInstance 使用稳定 ID。
5. Task 状态模型能够区分本地状态和远程状态。

4.7 Not Implemented Now

当前不实现：

* Node Discovery。
* Distributed Scheduler。
* Remote Task Migration。
* Multi-node Resource Scheduling。
* Distributed Consensus。

⸻

5. E02 — Resource Controller

Implementation note: QWE does not need to implement Docker or a container
runtime to provide Linux resource isolation. V0 already uses per-Session
systemd transient scopes and cgroup v2 for memory, swap, task-count, and CPU
limits. Stage 7 focuses on profiles, lifecycle hardening, observability,
long-run qualification, and careful expansion of controller support. See
[Stage 7 Resource Governance](stage7-resource-governance.md).

5.1 Objective

当前版本主要实现 Resource Monitoring。

未来可以扩展为 Resource Controller。

从只观察资源使用情况，逐步发展为对任务资源进行限制和管理。

5.2 Monitoring vs Control

需要明确两个不同职责：

Resource Management
       |
       +-- Resource Monitor
       |
       +-- Resource Controller

Monitor 负责采集。

Controller 负责执行资源策略。

5.3 Potential Resource Model

ResourcePolicy
{
    cpu_limit
    memory_limit
    io_limit
    gpu_visibility
    timeout
}

具体字段需要根据平台能力定义。

不能假设所有平台都支持所有限制。

5.4 Linux Backend

Linux 可以研究：

* cgroups v2。
* CPU Controller。
* Memory Controller。
* IO Controller。
* Process Tracking。

注意：

TaskInstance 是 QWE 的逻辑执行对象。

cgroup 是 Linux 的资源控制机制。

二者需要建立映射关系，但不能合并成同一个概念。

5.5 GPU Backend

GPU 需要独立研究。

候选：

* NVML。
* NVIDIA Container Toolkit。
* CUDA Runtime。
* 设备可见性控制。
* GPU 设备级和进程级统计。

需要明确：

CUDA_VISIBLE_DEVICES 不等于 GPU 显存限制。

Linux cgroups v2 也不提供通用 GPU 显存限制。

GPU 资源隔离必须依赖具体硬件、驱动和运行时能力。

5.6 Capability Model

建议未来采用能力描述：

ResourceCapabilities
{
    cpu_monitoring
    memory_monitoring
    cpu_limiting
    memory_limiting
    io_limiting
    gpu_monitoring
    gpu_isolation
}

某项能力不可用时，明确返回 Unsupported。

5.7 Current Architecture Reservation

当前：

* Resource Policy 归 TaskInstance。
* Resource Monitor 与 Process Manager 分离。
* 采集接口不绑定 Linux cgroups。
* GPU 统计保留 Device / Process / Task 三个层次。

⸻

6. E03 — Credential Broker

6.1 Objective

Credential Broker 用于管理和代理外部服务凭据。

典型场景：

* OpenAI API Key。
* Anthropic API Key。
* DeepSeek API Key。
* MiniMax API Key。
* 其他模型 Provider 凭据。

目标是避免将真实密钥直接暴露给 Task 子进程。

6.2 Problem

常见做法：

Task
  |
  +-- Environment Variable
          |
          +-- REAL_API_KEY

这种方式简单，但不能视为安全隔离。

真实密钥可能通过：

* 日志。
* 调试输出。
* 子进程信息。
* 崩溃转储。
* 环境信息。
* 错误处理。

被意外暴露。

不应将真实 Secret 放入 argv。

环境变量也不应被视为安全边界。

6.3 Potential Architecture

未来可以设计：

                 QWE CLI
                    |
                    v
             Credential Store
                    |
                    v
             Credential Broker
                    |
             Local Proxy Endpoint
                    |
             +------+------+
             |             |
             v             v
          Task A         Task B
             |             |
             +------+------+
                    |
                    v
               Provider API

Task 获得的是本地代理访问能力。

真实 API Key 保留在 Broker 内部。

6.4 Token Proxy

例如：

Task 使用：

OPENAI_BASE_URL=http://127.0.0.1:PORT
OPENAI_API_KEY=qwe-local-token

其中：

qwe-local-token 是本地代理凭据，不是真实 Provider API Key。

Broker 根据请求身份和授权策略决定是否转发。

6.5 Security Requirements

Broker 必须处理：

* Caller Authentication。
* Task Authorization。
* Provider Allowlist。
* Model Allowlist。
* Rate Limit。
* Quota。
* Token Revocation。
* Audit Log。
* Secret Redaction。

不能只因为 Task 连接的是 localhost，就认为请求可信。

6.6 Credential Storage

优先使用成熟方案：

* macOS Keychain。
* Linux Secret Service（需要考虑 Headless 环境）。
* age 等成熟加密工具。
* 系统级安全凭据存储。

不建议自行组合 Argon2id、AEAD 等算法实现完整 Vault。

需要注意：

Linux Secret Service 通常依赖用户会话环境。

Headless SSH Server 不能假设它一定可用。

6.7 Current Architecture Reservation

当前：

* 不在普通日志中记录 Secret。
* 配置文件支持 Secret Reference 的扩展空间。
* Task 定义不直接绑定具体 Provider Key。
* 不将真实 Secret 放入 argv。
* 不把环境变量当成安全隔离方案。

当前不实现完整 Credential Broker。

⸻

7. E04 — Security Sandbox

7.1 Objective

未来为 TaskInstance 提供隔离执行能力。

典型需求：

* 限制文件系统访问。
* 限制网络访问。
* 限制 CPU 和内存。
* 限制系统调用。
* 限制设备访问。
* 隔离不同任务。

7.2 Candidate Technologies

Linux：

* namespaces。
* cgroups v2。
* seccomp。
* Landlock。
* AppArmor。
* SELinux。
* Containers。

macOS：

* Sandbox 相关机制。
* App Sandbox。
* 虚拟化技术。

Windows：

* Job Objects。
* AppContainer。
* Windows Sandbox。

7.3 Sandbox Profile

未来可以定义：

SandboxProfile
{
    filesystem_policy
    network_policy
    process_policy
    resource_policy
    device_policy
}

Profile 描述期望能力。

具体执行由平台 Backend 负责。

7.4 Security Boundary

必须明确：

Resource Limit 不等于 Security Isolation。

限制 CPU 和内存不能阻止恶意进程访问不应访问的文件。

同样，使用容器也不自动意味着拥有完整的安全边界。

7.5 Current Architecture Reservation

当前：

* TaskInstance 作为隔离策略关联对象。
* Process Runtime 与 Sandbox Backend 分离。
* 不把 cgroup 直接写进通用 Task 模型。
* 资源限制和安全策略分别建模。

⸻

8. E05 — Agent Runtime

8.1 Objective

未来允许 QWE 统一管理 Agent 的运行环境。

例如：

* Claude Code。
* OpenCode。
* Codex。
* 自定义 Agent。
* 本地 LLM Agent。
* 多 Agent 协作任务。

QWE 不负责实现 Agent 的推理和决策逻辑。

它负责提供：

* 执行环境。
* 任务生命周期。
* 进程管理。
* 日志。
* 资源监控。
* 凭据访问。
* 环境配置。
* 任务恢复。

8.2 Potential Architecture

              QWE
               |
          Task Runtime
               |
          Agent Runtime
               |
      +--------+--------+
      |        |        |
      v        v        v
   Agent A  Agent B  Agent C
      |        |        |
      +--------+--------+
               |
          External Tools

8.3 Agent as Task

第一阶段可以将 Agent 看成普通 Task。

例如：

Task
{
    command
    arguments
    working_directory
    environment
}

Agent Runtime 不需要在当前版本成为独立核心对象。

未来根据需求再引入：

* Agent Definition。
* Agent Instance。
* Tool Permission。
* Context Store。
* Execution Trace。
* Approval Policy。

8.4 Agent Security

Agent 的主要风险不仅是进程管理。

还包括：

* Tool Permission。
* 文件系统访问。
* 网络访问。
* Credential Access。
* Prompt Injection。
* 工具调用审计。
* 人工确认。
* 任务中断和恢复。

未来需要结合 Sandbox 和 Credential Broker。

8.5 Current Architecture Reservation

当前：

* Task 支持任意外部命令。
* TaskInstance 有完整生命周期记录。
* 日志和事件可以关联 TaskInstance。
* 不把 Agent 逻辑放入 QWE 核心。
* 保留 Tool Permission 等未来扩展空间。

⸻

9. E06 — Environment Backend

9.1 Objective

未来实现更完整的开发环境描述和恢复能力。

典型场景：

* Python。
* uv。
* Conda。
* CUDA。
* Compiler。
* CMake。
* Meson。
* Node.js。
* Go。
* Rust。

9.2 Environment Definition

当前 Workspace Definition 可以包含：

EnvironmentDefinition
{
    project_root
    working_directory
    environment_variables
    runtime_metadata
    dependency_metadata
    toolchain_metadata
}

未来可增加：

EnvironmentBackend

用于支持不同环境管理方案。

9.3 Potential Backends

* uv。
* Conda。
* venv。
* Nix。
* Dev Containers。
* Docker。
* 自定义环境管理器。

QWE 不应重复实现所有环境管理工具。

9.4 Environment Reproducibility

需要区分：

* Environment Description。
* Environment Creation。
* Environment Activation。
* Environment Verification。
* Environment Restoration。

记录环境信息不代表能够完整复现环境。

例如，记录 CUDA 版本和 Python 包版本，并不能保证底层驱动、系统库和硬件完全一致。

9.5 Current Architecture Reservation

当前：

* Workspace 作为配置定义。
* Task 支持 Working Directory 和 Environment。
* 环境元数据与运行时状态分离。
* 不把 uv/Conda 等工具绑定到 Task Runtime。

⸻

10. E07 — Terminal Backend

10.1 Objective

未来支持不同终端实现。

包括：

* PTY Backend。
* Terminal Emulator Backend。
* Terminal Renderer。
* Terminal Input/Output Backend。

10.2 Architecture

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

不同 Backend 不应承担相同职责。

例如：

PTY Backend 负责操作系统 PTY。

Terminal Emulator Backend 负责 VT 状态解析和终端状态维护。

Session Manager 负责 Session 生命周期。

10.3 Candidate Components

Rust：

* alacritty_terminal。
* portable-pty。

C/C++：

* libvterm。
* POSIX PTY。
* libuv。

Go：

* PTY 库。
* 外部 VT Parser 或终端状态实现。

10.4 Important Risks

需要验证：

* Terminal Emulator 状态能否持久化。
* Resize 后状态如何同步。
* 多客户端 Attach/Detach 如何处理。
* Daemon 重启后如何恢复。
* Unicode 和宽字符是否正确。
* Alternate Screen 如何处理。
* Shell Job Control 是否符合预期。

10.5 Current Architecture Reservation

当前：

* PTY 与 VT Parser 分离。
* Terminal Model 与 Execution Model 分离。
* Pane 不拥有 TaskInstance。
* Session Lease 不影响 Task 生命周期。

⸻

11. Cross-Extension Dependencies

未来扩展之间存在依赖关系。

例如：

                  QWE Core
                     |
        +------------+------------+
        |            |            |
        v            v            v
   Resource       Credential    Environment
   Controller     Broker        Backend
        |            |            |
        +------------+------------+
                     |
                     v
                Agent Runtime
                     |
                     v
                Security Sandbox
                     |
                     v
                  QWE Mesh

这只是概念依赖图，不代表必须按这个顺序实现。

更准确的依赖关系：

Extension	Depends On
QWE Mesh	Stable Task Model、IPC、Node Identity
Resource Controller	TaskInstance、Process Manager
Credential Broker	Task Identity、IPC、Authorization
Security Sandbox	TaskInstance、Process Runtime
Agent Runtime	Task Runtime、Credential Broker、Sandbox
Environment Backend	Workspace Definition、Task Definition
Terminal Backend	Terminal Model、PTY Interface

需要注意：

QWE Mesh 不应成为所有扩展的前置条件。

单机 QWE 应长期保持独立可用。

⸻

12. Extension Design Registry

每个扩展方向都必须维护统一的设计记录。

建议格式：

id: E01
name: QWE Mesh
status: research
problem:
  - Remote task execution
  - Multi-node management
proposed_architecture:
  - Local daemon
  - Remote node
  - Remote execution protocol
core_model_impact:
  - ExecutionTarget
  - NodeIdentity
reserved_interfaces:
  - ExecutionTarget
  - RemoteTaskBackend
dependencies:
  - Task Runtime
  - IPC
  - Persistence
security_requirements:
  - Mutual Authentication
  - Authorization
  - Audit
current_exclusions:
  - Distributed Scheduler
  - Task Migration
implementation_trigger:
  - Actual requirement for remote GPU execution
  - Multiple QWE nodes need unified management
risks:
  - Network partition
  - State synchronization
  - Identity management

所有扩展设计均应回答：

1. 解决什么问题？
2. 典型使用场景是什么？
3. 对核心对象模型有什么影响？
4. 需要依赖哪些模块？
5. 是否需要新增 Backend？
6. 当前需要预留什么？
7. 当前为什么不实现？
8. 什么情况下应该启动实现？
9. 有哪些安全风险？
10. 有哪些尚未验证的技术假设？

⸻

13. Architecture Evolution Rules

未来新增功能时，按以下原则判断是否需要修改核心架构。

13.1 Additive Change

如果新功能只需要新增 Backend 或模块，不改变现有对象语义：

优先采用 Additive Change。

例如：

新增 Resource Controller。

13.2 Model Change

如果新功能需要改变核心对象关系：

必须先更新 Design Baseline。

例如：

TaskInstance 增加 Remote Execution Target。

13.3 Breaking Change

如果新功能导致原有接口和状态语义无法兼容：

需要明确：

* 旧接口如何迁移。
* 旧数据如何兼容。
* Daemon 如何升级。
* 运行中的任务如何处理。
* 是否需要版本化。

13.4 Avoid Hidden Coupling

不允许为了快速实现功能，在多个模块中直接访问彼此的内部数据结构。

需要通过明确接口交换状态。

但不要求第一阶段把每个模块都抽象成独立服务。

⸻

14. Current Architecture Reservation Checklist

第一阶段开发前，应确认以下事项。

ID	Reservation	Required
R01	TaskInstance 使用稳定 ID	Yes
R02	Task 与 TaskInstance 分离	Yes
R03	Terminal Model 与 Execution Model 分离	Yes
R04	Workspace 是配置定义	Yes
R05	TaskInstance 可关联 ExecutionTarget	Research / Minimal Reservation
R06	Resource Monitor 与 Resource Controller 分离	Yes
R07	PTY 与 Terminal Emulator 分离	Yes
R08	Process Identity 不只依赖 PID	Yes
R09	IPC 协议有版本演进空间	Yes
R10	Task 日志和结构化事件可关联	Yes
R11	Secret 不直接写入普通配置和日志	Yes
R12	不绑定 Linux 专有资源控制接口	Yes
R13	扩展模块不能反向控制 QWE 核心对象生命周期	Yes

⸻

15. Final Position

QWE 的扩展设计应保持两个层次：

第一层：

当前版本真正需要实现的核心能力。

第二层：

经过研究、明确未来方向的架构扩展能力。

两者不能混为一谈。

当前不需要：

* 一个通用插件市场。
* 一个通用分布式调度器。
* 一个完整 Vault。
* 一个完整容器管理系统。
* 一个完整 Agent Framework。

但当前必须避免：

* TaskInstance 被硬编码为本地 PID。
* Workspace 被设计成运行时生命周期对象。
* Pane 成为 Task 的所有者。
* Resource Monitor 与 Linux cgroups 强绑定。
* PTY 和 VT Parser 混为一谈。
* Daemon 成为所有 Task 的生命周期唯一依赖。
* 真实 Secret 直接进入普通任务配置。

QWE 的目标不是提前实现未来，而是避免今天的设计封死明天的选择。
