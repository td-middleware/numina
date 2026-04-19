# Numina 飞书 Channel 使用指南

通过飞书 Channel，你可以让 Numina 作为飞书机器人运行，接收飞书消息并自动用 AI 回复。

---

## 工作原理

```
飞书用户发消息
      ↓
飞书服务器推送事件
      ↓
lark-cli event +subscribe（WebSocket 长连接）
      ↓
Numina 解析消息 → ReAct Agent Loop
      ↓
lark-cli 回复消息给用户
```

Numina 通过调用 `lark-cli event +subscribe` 订阅飞书 WebSocket 事件流，实时接收消息，触发 AI 处理后自动回复。

---

## 前置条件

### 1. 安装并配置 lark-cli

```bash
# 安装 lark-cli（如果还没安装）
npm install -g @larksuiteoapi/lark-cli
# 或
brew install lark-cli
```

### 2. 配置飞书机器人应用

在 [飞书开放平台](https://open.feishu.cn/app) 创建企业自建应用：

**必须开启的权限（Scopes）：**
- `im:message` — 接收消息
- `im:message:send_as_bot` — 发送消息
- `im:message.group_at_msg:readonly` — 接收群聊 @ 消息

**必须订阅的事件：**
- `im.message.receive_v1` — 接收消息事件

**机器人设置：**
- 在「机器人」页面启用机器人功能
- 配置消息卡片请求网址（如果需要）

### 3. 登录 lark-cli（机器人身份）

```bash
# 配置应用凭证
lark-cli config init

# 以机器人身份登录
lark-cli auth login --as bot

# 验证登录状态
lark-cli auth status
```

---

## 快速开始

### 启动飞书 Channel（React 模式）

```bash
numina channel lark
```

启动后终端会显示：
```
⚡ 飞书 Channel 启动（React 模式，实时处理）
📡 正在连接飞书 WebSocket 事件流...
   过滤规则：仅处理私聊消息 和 群聊中 @机器人 的消息
   按 Ctrl+C 停止
```

### 测试

1. 在飞书中找到你的机器人
2. 给机器人发一条私聊消息，例如：`你好，帮我查一下今天的天气`
3. 机器人会自动用 AI 回复

---

## 消息过滤规则

| 消息类型 | 是否处理 |
|---------|---------|
| 私聊消息（p2p） | ✅ 处理 |
| 群聊中 @机器人 | ✅ 处理 |
| 群聊中未 @机器人 | ❌ 忽略 |
| 系统消息/通知 | ❌ 忽略 |
| 空内容消息 | ❌ 忽略 |

---

## 处理模式

### React 模式（默认）

来一条消息立即触发 ReAct Agent Loop，实时处理并回复。

```bash
numina channel lark
```

**适合场景：** 实时问答、任务执行、代码助手

### Buffer 模式

将消息存入缓冲区，每隔指定秒数批量处理。

```bash
# 每 60 秒批量处理一次
numina channel lark --buffer 60

# 每 5 分钟批量处理一次
numina channel lark --buffer 300
```

**适合场景：** 消息汇总、定期报告、批量分析

---

## 完整命令参数

```bash
numina channel lark [OPTIONS]

OPTIONS:
  --buffer <SECONDS>     Buffer 模式，指定批量处理间隔（秒）
                         不指定则使用 React 模式（默认）

  --cli-path <PATH>      指定 lark-cli 可执行文件路径
                         默认使用 PATH 中的 lark-cli

  -m, --model <MODEL>    指定使用的 AI 模型（覆盖默认配置）

  [EXTRA_ARGS]...        额外传递给 lark-cli 的参数
```

### 示例

```bash
# 基本启动
numina channel lark

# 指定模型
numina channel lark -m claude-3-5-sonnet-20241022

# 指定 lark-cli 路径
numina channel lark --cli-path /usr/local/bin/lark-cli

# Buffer 模式（每 2 分钟批量处理）
numina channel lark --buffer 120

# 查看 channel 状态
numina channel status
```

---

## 后台运行（生产环境）

### 使用 nohup

```bash
nohup numina channel lark > ~/.numina/logs/channel.log 2>&1 &
echo $! > ~/.numina/channel.pid
```

停止：
```bash
kill $(cat ~/.numina/channel.pid)
```

### 使用 systemd（Linux）

创建 `/etc/systemd/system/numina-channel.service`：

```ini
[Unit]
Description=Numina Lark Channel
After=network.target

[Service]
Type=simple
User=your_user
ExecStart=/usr/local/bin/numina channel lark
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable numina-channel
sudo systemctl start numina-channel
sudo systemctl status numina-channel
```

### 使用 launchd（macOS）

创建 `~/Library/LaunchAgents/com.numina.channel.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.numina.channel</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOUR_USER/.cargo/bin/numina</string>
        <string>channel</string>
        <string>lark</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/Users/YOUR_USER/.numina/logs/channel.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/YOUR_USER/.numina/logs/channel-error.log</string>
</dict>
</plist>
```

```bash
mkdir -p ~/.numina/logs
launchctl load ~/Library/LaunchAgents/com.numina.channel.plist
launchctl start com.numina.channel
```

---

## 开启调试日志

```bash
# 查看详细日志（包括消息解析、过滤过程）
RUST_LOG=debug numina channel lark

# 只看 channel 相关日志
RUST_LOG=numina::channel=debug numina channel lark
```

---

## 常见问题

### Q: 机器人没有回复消息

**检查步骤：**

1. 确认 lark-cli 已正确配置：
   ```bash
   lark-cli auth status
   ```

2. 确认机器人权限已开启（`im:message:send_as_bot`）

3. 确认消息类型符合过滤规则（私聊 或 群聊 @机器人）

4. 开启调试日志查看详情：
   ```bash
   RUST_LOG=debug numina channel lark
   ```

### Q: 连接断开后自动重连吗？

是的。Numina 内置自动重连机制：
- lark-cli 进程退出后，等待 **5 秒**自动重启
- lark-cli 启动失败时，等待 **10 秒**后重试
- 按 `Ctrl+C` 才会真正停止

### Q: 群聊中如何触发机器人？

在群聊消息中 **@机器人** 即可触发，例如：
```
@Numina 帮我分析一下这段代码
```

### Q: 如何让机器人只处理特定群的消息？

目前 Numina 处理所有 @机器人 的群聊消息。如需过滤特定群，可以通过 Skills 实现自定义过滤逻辑。

### Q: 消息内容支持哪些格式？

| 消息类型 | 支持情况 |
|---------|---------|
| 文本（text） | ✅ 完整支持 |
| 富文本（post） | ✅ 提取纯文本 |
| 图片（image） | ⚠️ 识别类型，内容待扩展 |
| 文件（file/audio/media） | ⚠️ 识别类型，内容待扩展 |
| 消息卡片（interactive） | ⚠️ 识别类型，内容待扩展 |

---

## 架构说明

```
numina channel lark
        │
        ▼
  LarkChannel.run()
        │
        ├── 启动子进程：lark-cli event +subscribe
        │   --event-types im.message.receive_v1
        │   --compact --quiet --as bot
        │
        ├── 逐行读取 NDJSON 输出
        │
        ├── 过滤：私聊 或 @机器人
        │
        ▼
  ChannelDispatcher（消息队列）
        │
        ├── React 模式 → ReactHandler
        │       │
        │       ├── 意图分析（是否需要澄清）
        │       ├── ChatEngine.chat_react()
        │       └── lark-cli api POST /reply
        │
        └── Buffer 模式 → BufferHandler
                │
                └── 定时批量 → ChatEngine.chat_once()
```
