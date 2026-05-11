# Eggs Desktop Companion Plugin for Claude Code

将 Claude Code 与 Eggs 动画桌面宠物集成，实时显示 Claude 的工作状态。

## 功能特性

### Hooks 集成
- 🗣️ **用户提示通知** - 当你向 Claude 提交问题时显示
- 🔧 **工具使用通知** - 实时显示 Claude 执行的操作（读文件、写文件、运行命令等）
- 🔐 **权限请求通知** - 当 Claude 需要权限时提醒你
- 💬 **响应完成通知** - Claude 完成回复时显示消息

### Skill 集成
- 🐾 **桌面宠物管理** - 通过 `/eggs` 命令启动、停止、管理动画桌面宠物
- 🎨 **状态控制** - 切换宠物状态（idle、running、waving、jumping 等）
- 🔄 **宠物切换** - 在不同的宠物角色之间切换
- 🌐 **远程互动** - 可选的远程宠物互动功能
- 🛠️ **精灵图工具** - 处理、提取、验证、合并桌面宠物精灵图（仅 macOS）

## 安装方法

### 方式一：本地测试

```bash
# 在项目目录中测试
claude --plugin-dir ./eggs-claude-plugin
```

### 方式二：全局安装

```bash
# 复制到 Claude plugins 目录
cp -r eggs-claude-plugin ~/.claude/plugins/eggs

# 在 ~/.claude/settings.json 中启用
{
  "plugins": ["eggs"]
}
```

### 方式三：项目级安装

在项目的 `.claude/settings.json` 中配置：

```json
{
  "plugins": ["eggs-companion"]
}
```

## 快速开始

### 启动桌面宠物

在 Claude Code 中输入：

```
/eggs
```

或者直接说："启动桌面宠物"

首次运行会自动下载预编译的二进制文件（约 10 MB）并缓存到 `~/.eggs/bin/`。

### 基本命令

```bash
# 启动宠物
/eggs start

# 停止宠物
/eggs stop

# 查看状态
/eggs status

# 重启宠物
/eggs restart

# 改变宠物状态
/eggs state idle
/eggs state running
/eggs state waving
/eggs state jumping

# 切换宠物
/eggs pet builtin kebo-a
/eggs pet local noir-webling
```

## 工作原理

该插件通过 Claude Code 的 hooks 系统监听以下事件：

1. **UserPromptSubmit** - 用户提交提示时触发
2. **PostToolUse** - Claude 执行工具后触发
3. **PermissionRequest** - Claude 请求权限时触发
4. **Stop** - Claude 完成响应时触发

每个事件都会：
1. 尝试通过 IPC 直接发送给 Eggs 进程（如果正在运行）
2. 如果 IPC 失败，写入 bubble-spool 文件供 Eggs 轮询读取

## 依赖要求

- **Node.js** - 用于运行 hook 脚本
- **curl 或 wget** - 用于下载 Eggs 二进制文件（首次运行）
- **macOS / Linux / Windows** - 支持所有主流平台
  - macOS: Universal binary (Intel + Apple Silicon)
  - Linux: x86_64 / arm64
  - Windows: x86_64

**无需 Python、编译器或其他运行时依赖！**

## 配置

Hook 脚本会自动查找 Eggs 可执行文件，按以下顺序：

1. `desktop/src-tauri/target/debug/eggs` (开发版本)
2. `~/.eggs/bin/eggs` (已安装版本)
3. `desktop/src-tauri/target/release/eggs` (发布版本)
4. PATH 环境变量中的 `eggs`

你可以通过设置 `EGGS_BIN_DIR` 环境变量来自定义查找路径。

## 消息格式

插件会智能格式化不同类型的操作：

- **Read file** - 读取文件操作
- **Write file** - 写入/编辑文件操作
- **Search** - 搜索/grep 操作
- **Web fetch** - 网络请求
- **Web search** - 网络搜索
- **Run command** - 执行命令
- **MCP tool** - MCP 工具调用

## 精灵图工具（仅 macOS）

插件包含了一套 Swift 工具用于处理桌面宠物的精灵图。

### 构建工具

```bash
./skill/tools/build_tools.sh
```

工具会被安装到 `~/.eggs/bin/`。

### 使用工具

**提取带边框的网格精灵图：**
```bash
~/.eggs/bin/extract_sprite input.png output-dir
```

**提取无边框的规则网格：**
```bash
~/.eggs/bin/extract_sprite input.png output-dir \
  --grid uniform \
  --columns 8 \
  --rows 9
```

**强制统一帧尺寸：**
```bash
~/.eggs/bin/extract_sprite input.png output-dir --frame-size 251
```

**合并多个精灵图：**
```bash
~/.eggs/bin/merge_spritesheets output-dir sheet-a.json sheet-b.json
```

## 远程互动（可选）

Eggs 支持可选的远程宠物互动功能。

### 配置远程服务器

```bash
/eggs remote server http://localhost:8787
/eggs remote upload kebo-a
/eggs remote
```

### 房间模式

```bash
# 创建/加入房间
/eggs remote room ABC123

# 离开房间
/eggs remote leave

# 禁用远程功能
/eggs remote off
```

## 故障排除

### Hook 没有触发

1. 检查 Eggs 是否正在运行：`ps aux | grep eggs`
2. 检查 hook 脚本是否可执行：`ls -l eggs-claude-plugin/hooks/eggs-codex-notify.js`
3. 查看 Claude Code 的 hooks 状态：在 Claude 中输入 `/hooks`

### 消息没有显示

1. 检查 bubble-spool 目录：`ls ~/.eggs/bubble-spool/`
2. 检查 Eggs 日志（如果有）
3. 尝试手动运行 hook 脚本测试：
   ```bash
   echo '{"prompt":"test"}' | node eggs-claude-plugin/hooks/eggs-codex-notify.js user_prompt_submit
   ```

### 宠物无法启动

1. 确保使用非沙箱模式启动（macOS 需要访问 WindowServer）
2. 检查是否有权限问题：`ls -l ~/.eggs/bin/eggs`
3. 手动测试启动：`~/.eggs/bin/eggs start`
4. 查看下载的二进制文件：`ls -lh ~/.eggs/bin/`

### 首次下载失败

1. 检查网络连接
2. 确保 curl 或 wget 可用：`which curl wget`
3. 手动下载并放置到 `~/.eggs/bin/eggs`
4. 设置自定义下载源：`export EGGS_RELEASE_URL=<your-mirror>`

## 目录结构

```
eggs-claude-plugin/
├── .claude-plugin/
│   └── plugin.json          # 插件清单（包含 skill 注册）
├── hooks/
│   ├── hooks.json           # Hooks 配置
│   └── eggs-codex-notify.js # Hook 脚本
├── skill/
│   ├── SKILL.md             # Skill 定义
│   ├── eggs                 # POSIX 启动器（macOS/Linux）
│   ├── eggs.cmd             # Windows 启动器
│   ├── agents/              # 代理配置
│   ├── scripts/             # 旧版 Python/Swift 脚本
│   └── tools/               # 精灵图处理工具（Swift）
├── .gitignore
└── README.md
```

## 开发与自定义

### 修改 Hook 行为

编辑 `hooks/eggs-codex-notify.js`：

```javascript
// 修改消息格式
function formatHookMessage(payload) {
  // 你的自定义逻辑
}

// 修改消息长度限制
function shorten(text, limit = 110) {
  // 默认 110 字符
}
```

### 创建自定义宠物

1. 准备精灵图（8x9 网格，每帧 192x208 像素）
2. 创建 `pet.json` 配置文件
3. 安装到 `~/.eggs/pets/`：
   ```bash
   /eggs install /path/to/your-pet-dir
   ```
4. 切换到你的宠物：
   ```bash
   /eggs pet local your-pet-name
   ```

### 环境变量

- `EGGS_RELEASE_URL` - 自定义下载源（默认：GitHub Releases）
- `EGGS_BIN_DIR` - 自定义缓存目录（默认：`~/.eggs/bin`）
- `EGGS_APP_DIR` - 自定义应用数据目录（默认：`~/.eggs`）
- `EGGS_VERIFY_INTERVAL` - SHA256 校验间隔秒数（默认：600）
- `EGGS_SKIP_VERIFY=1` - 跳过版本校验（离线/CI 模式）

## 许可证

MIT License

## 作者

Alex.Liu
