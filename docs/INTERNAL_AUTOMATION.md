# 本机自动化客户端

`automation-client` 提供内部自用的 `meetily-internal` CLI 和 MCP stdio 服务。它只读取本机 Meetily 当前用户的会议、转录分页、已保存摘要及任务状态；不会提供录音、写入、导出、文件路径或模型密钥工具。Meetily 必须已打开，并在设置中显式启用本机自动化。

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

## 验证

```sh
pnpm --dir automation-client install --frozen-lockfile
pnpm --dir automation-client build
pnpm --dir automation-client test
```

测试使用临时 discovery 与 stub HTTP 响应，不读取用户会议或既有 Keychain 项。连接到真实助手并验证其主机配置，需要由用户选择目标助手并在其本机完成后续验收。
