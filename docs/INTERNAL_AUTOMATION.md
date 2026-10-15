# 本机自动化客户端

`automation-client` 提供内部自用的 `meetily-internal` CLI 和 MCP stdio 服务。它读取本机 Meetily 当前用户的会议、转录分页、已保存摘要及任务状态；摘要任务和录音控制仅在单独的 write/record 客户端授权下可用。不会提供导出、文件路径或模型密钥工具。Meetily 必须已打开，并在设置中显式启用本机自动化。

## 凭据与连接

首次创建 read 客户端时，Meetily 仅显示一次令牌，并将其保存在 macOS Keychain。客户端从私有 discovery 文件读取随机 loopback 端口，再核对协议版本和活动实例 ID；过期或非 `127.0.0.1` 地址会被拒绝。读取凭据使用 macOS `security` 命令，令牌不放入命令参数或输出。

默认 discovery 路径为 `~/Library/Application Support/com.meetily.ai/automation/discovery.json`。设置 `MEETILY_AUTOMATION_CLIENT_ID` 为 Meetily 显示的客户端 ID，客户端即可从 Keychain 读取令牌。测试时可用 `MEETILY_AUTOMATION_DISCOVERY` 指向临时 discovery 文件；短期本机开发也可通过进程环境变量 `MEETILY_AUTOMATION_TOKEN` 传令牌，避免将其放进 shell 命令行、配置文件、版本控制或日志。环境令牌会被继承给子进程，优先使用 Keychain。

## CLI

构建后，从仓库根目录执行：

```sh
pnpm --dir automation-client install --frozen-lockfile
pnpm --dir automation-client build
MEETILY_AUTOMATION_CLIENT_ID='<client-id>' node automation-client/dist/cli.js meetings --limit 50
MEETILY_AUTOMATION_CLIENT_ID='<client-id>' node automation-client/dist/cli.js search '预算回顾' --limit 20
MEETILY_AUTOMATION_CLIENT_ID='<client-id>' node automation-client/dist/cli.js transcript '<meeting-id>' --limit 100
MEETILY_AUTOMATION_CLIENT_ID='<client-id>' node automation-client/dist/cli.js summary '<meeting-id>'
MEETILY_AUTOMATION_CLIENT_ID='<client-id>' node automation-client/dist/cli.js job '<job-id>'
```

结果 JSON 写到 stdout，错误写到 stderr。退出码为 `0` 成功、`1` 请求错误、`2` 参数错误、`3` Meetily 关闭/未启用/实例失效、`4` 凭据缺失/拒绝。每次调用最多返回一页；用 `next_cursor` 继续，不会自动拉取整个账户。

## MCP stdio

将以下条目手动添加到你选择的 MCP 主机配置，并替换绝对仓库路径与客户端 ID。此步骤不会覆盖或自动修改主机配置：

```json
{
  "mcpServers": {
    "meetily-internal": {
      "command": "node",
      "args": ["/absolute/path/to/meetily/automation-client/dist/mcp.js"],
      "env": {
        "MEETILY_AUTOMATION_CLIENT_ID": "<client-id>"
      }
    }
  }
}
```

服务在 stdio 上只输出 MCP JSON-RPC 消息，诊断信息使用 stderr。注册的五个只读工具为 `list_meetings`、`search_meetings`、`get_transcript`、`get_summary`、`get_job`。它们按页返回，不自动扩展分页；主机关闭 Meetily 后会收到明确的本机实例不可用错误。

### 主机兼容性

ChatGPT 自定义 MCP app 不直接连接本机 stdio 服务。OpenAI Secure MCP Tunnel 可让本机 `tunnel-client` 通过出站 HTTPS 连接 OpenAI，再把 MCP 请求转发给本机 stdio 命令；Meetily 服务仍只监听 loopback，不需开放入站端口或公网地址。连接需要 Platform `tunnel_id`、供 `tunnel-client` 使用的 runtime API key、Platform 组织的 Tunnels Read + Use 权限，以及 ChatGPT 工作区允许添加自定义 MCP server；创建或修改 tunnel 另需 Tunnels Read + Manage。[Secure MCP Tunnel 指南](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) · [Platform Tunnels 设置](https://platform.openai.com/settings/organization/tunnels) · [ChatGPT MCP 开发者模式说明](https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt)

`tunnel_id` 获取路径：登录拥有该 tunnel 的 OpenAI Platform 组织，打开 [Platform Tunnels 设置](https://platform.openai.com/settings/organization/tunnels)，在 tunnel 列表中复制 ID。若列表为空，需要有 Tunnels Read + Manage 权限的组织 Owner/RBAC 管理员创建 tunnel；并确认在 tunnel 的关联设置中包含目标 ChatGPT 工作区，否则工作区创建自定义 MCP app 时可能看不到它。只有运行或使用现有 tunnel 时，Tunnels Read + Use 即可；不要把 runtime API key 发到聊天或写入仓库。

Meetily MCP 客户端当前仅公开五个只读工具，适合 ChatGPT Pro 的 read/fetch 权限。摘要写入、任务取消和录音控制不通过 Pro MCP 提供；OpenAI 文档目前将完整 MCP 写操作列为 Business、Enterprise 和 Edu 工作区的 beta 能力。

### 用 Secure MCP Tunnel 启动本机只读服务

从[官方 tunnel-client 发布页](https://github.com/openai/tunnel-client/releases)安装 `tunnel-client`，在 ChatGPT 工作区创建/授权自定义 MCP app，并在 Platform 组织中取得 Tunnels Read + Use 权限。运行以下准备命令；把示例值替换为 Platform Tunnels 设置中的 tunnel ID 与 Meetily 设置页显示的只读 client ID：

```sh
pnpm --dir automation-client install --frozen-lockfile
pnpm --dir automation-client build
export CONTROL_PLANE_TUNNEL_ID='tunnel_...'
export MEETILY_AUTOMATION_CLIENT_ID='...'
scripts/meetily-chatgpt-tunnel.sh
```

启动器会在当前终端静默提示输入 OpenAI runtime API key；也可由调用进程安全注入 `CONTROL_PLANE_API_KEY`。key 不放在命令参数或配置文件中。脚本以 `--mcp.command` 启动已构建的 Meetily MCP stdio 服务，客户端再从 macOS Keychain 读取对应的 Meetily read token。Meetily 必须保持运行并启用 Local automation。每个 tunnel ID 同时只能运行一个使用 stdio binding 的 `tunnel-client` 实例；结束进程即可断开。自动化测试使用假的 tunnel-client 校验参数与 key 隔离，不能代替真实 Platform/ChatGPT 连通验收。[Secure MCP Tunnel 操作说明](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) · [tunnel-client 官方说明](https://github.com/openai/tunnel-client)

## 原生 HTTP 停录与超时重试

持有 `record` 权限的客户端可调用 `POST /api/internal/v1/recordings/{session_id}/stop`，并提供 `Idempotency-Key`。成功响应保持 `{"session_id":"...","status":"stopped"}`。本节描述原生 HTTP API；上述五个只读 MCP 工具不提供录音控制。

网关仍有20秒请求期限。停录一旦被独立任务接收，幂等登记、原生停录收尾和成功结果落库由该任务持续执行；HTTP超时、调用方取消等待或关闭自动化入口不会取消这项已接收操作。超时响应为 `503 / BUSY / Request deadline exceeded`，它不代表停录已失败或完成。客户端应保留原 session ID 和原 key，稍后以相同请求重试：处理中仍返回 `BUSY`，完成后返回已持久化的同一成功结果，不重复停录。

其他录音开始/停止操作占用录音锁时，新停录 action 立即返回 `BUSY`，不会长期排队等待。获得锁后仍核对 session，旧 session 返回 `CONFLICT`。普通业务错误按既有合同清理幂等登记；如果成功结果落库失败或原生 action panic，则保留未决登记以避免重复副作用，客户端应检查应用状态与恢复材料，不应自动更换 key 重发。独立任务只在当前应用进程及运行时存活期间继续执行，不提供应用退出或崩溃后的完成保证。

## Webhook 接收

在 Meetily 的“设置 → Local automation”中添加目标。地址必须为 HTTPS，并输入与URL主机完全匹配的批准项。Meetily 首次解析时校验并固定解析IP；每次投递会重新解析并拒绝未批准的新IP，不使用系统HTTP代理，也不跟随重定向。只有确实需要内网接收器时，才同时录入目标主机和精确IP。

创建订阅时会生成随机签名密钥，写入 macOS Keychain，界面仅显示一次。接收端应把密钥放在自己的秘密存储中。请求正文是UTF-8 JSON，包含 `event_id`、`meeting_id`、`kind`、`revision`、`job_id` 和 `occurred_at`，不包含转录/摘要正文。请求头为 `X-Meetily-Event`、`X-Meetily-Timestamp` 和 `X-Meetily-Signature: sha256=<hex>`。签名输入为ASCII十进制时间戳、一个句点、随后是未经JSON重序列化的原始请求字节；使用HMAC-SHA256并以常量时间比较。接收端还应限制时间戳年龄，并将 `event_id` 作为唯一键落库，以处理重复投递。

目前支持 `recording.stopped`、`transcript.completed`、`summary.completed` 与 `summary.failed`。摘要/转写事务提交后写入事件和各订阅投递行；故障后队列继续重试。2xx确认成功，429/5xx/网络问题最多再试5次，其他4xx终止；停用订阅会暂停待投递，重新启用可继续。`recording.stopped` 只说明录音文件已保存，不代表转写已完成。

## 验证

```sh
pnpm --dir automation-client install --frozen-lockfile
pnpm --dir automation-client build
pnpm --dir automation-client test
```

测试使用临时 discovery 与 stub HTTP 响应，不读取用户会议或既有 Keychain 项。连接到真实助手并验证其主机配置，需要由用户选择目标助手并在其本机完成后续验收。
