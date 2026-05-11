# Eggs Claude Plugin 安装指南

## 快速安装

### 方式一：全局安装（推荐）

```bash
# 1. 复制插件到 Claude plugins 目录
cp -r eggs-claude-plugin ~/.claude/plugins/eggs

# 2. 在 ~/.claude/settings.json 中启用插件
# 如果文件不存在，创建它：
cat > ~/.claude/settings.json << 'EOF'
{
  "plugins": ["eggs"]
}
EOF

# 如果文件已存在，手动添加 "plugins": ["eggs"]
```

### 方式二：项目级安装

```bash
# 在项目的 .claude/settings.json 中配置
mkdir -p .claude
cat > .claude/settings.json << 'EOF'
{
  "plugins": ["eggs-companion"]
}
EOF
```

### 方式三：临时测试

```bash
# 直接从插件目录启动 Claude Code
claude --plugin-dir ./eggs-claude-plugin
```

## 验证安装

启动 Claude Code 后，输入：

```
/eggs
```

如果看到桌面上出现动画宠物，说明安装成功！

## 首次运行

首次运行 `/eggs` 时，会自动：
1. 下载适合你系统的预编译二进制文件（约 10 MB）
2. 缓存到 `~/.eggs/bin/eggs`
3. 启动桌面宠物

下载过程可能需要几秒到几十秒，取决于网络速度。

## 卸载

```bash
# 删除插件
rm -rf ~/.claude/plugins/eggs

# 删除缓存的二进制文件和数据
rm -rf ~/.eggs

# 从 settings.json 中移除 "eggs"
```

## 故障排除

### 插件未加载

检查 Claude Code 是否识别到插件：
```bash
# 查看 Claude Code 日志
claude --version
```

### 无法下载二进制文件

如果网络受限，可以手动下载：

1. 访问 https://github.com/larchliu/eggs/releases/latest
2. 下载对应平台的文件：
   - macOS: `eggs-darwin-universal`
   - Linux x86_64: `eggs-linux-x86_64`
   - Linux ARM64: `eggs-linux-arm64`
   - Windows: `eggs-windows-x86_64.exe`
3. 重命名并放置到 `~/.eggs/bin/eggs`（Windows 为 `eggs.exe`）
4. 添加执行权限：`chmod +x ~/.eggs/bin/eggs`

### macOS 安全提示

首次运行时，macOS 可能提示"无法验证开发者"：

1. 打开"系统偏好设置" > "安全性与隐私"
2. 点击"仍要打开"
3. 或者在终端运行：`xattr -d com.apple.quarantine ~/.eggs/bin/eggs`

## 下一步

查看 [README.md](README.md) 了解完整功能和使用方法。
