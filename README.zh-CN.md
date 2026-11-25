<div align="center">

<img src="assets/app-icon.svg" alt="anterm" width="88" height="88" />

# anterm

**GPU 加速的现代化终端工作台：会话常驻、原生 SSH/SFTP、内置代码编辑器与一线 Coding Agent 支持。**

<sub>纯 Rust · 基于 GPUI Kit GPU 渲染 · Alacritty VT 内核 · 无头常驻会话守护进程</sub>

<br />

[![CI](https://github.com/netkn001/anterm/actions/workflows/ci.yml/badge.svg)](https://github.com/netkn001/anterm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/netkn001/anterm?label=release&color=3FDD8C)](https://github.com/netkn001/anterm/releases)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-blue)](https://github.com/netkn001/anterm/releases)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

<sub>[English](README.md) · 简体中文</sub>

</div>

---

## 为什么选择 anterm？

在 **anterm** 中，shell 与窗格由轻量的无头后台守护进程（`anterm-server`）持有，而不是绑定在 GUI 窗口上。核心体验都建立在这一架构之上：

- **无需 tmux 的会话常驻**：关闭窗口后 shell 照常运行；重启应用后自动恢复分屏布局、屏幕历史以及已支持的 agent 会话。
- **GPU 渲染与高频隔离**：纯 Rust 构建于 [GPUI Kit](https://gpui-kit.com) 与 [`alacritty_terminal`](https://github.com/alacritty/alacritty) 之上；高频 PTY 刷新严格隔离在 `TerminalCanvas`，文件树、Git 提交图、SFTP 列表与侧边栏全量虚拟化渲染。
- **原生 Agent 感知**：内置识别 28 款 CLI Coding Agent，提供品牌图标、实时状态点、Git 分支与 diff 摘要、等待输入通知、托盘提醒、重启续接与会话 Fork。
- **编辑器级终端输入**：基于历史的内联补全建议、附带说明的 Tab 补全、语法高亮、多行输入与停靠式 Composer（<kbd>⌘ I</kbd>）、可选 Helix 模态编辑、模糊历史搜索（<kbd>⌃ R</kbd>），无需安装任何 shell 插件。
- **原生 SSH 与远程开发**：内置纯 Rust `russh` 协议栈，凭据存入系统钥匙串，自带 SFTP 文件浏览器、本地/远程/动态端口转发、跳板机以及无需 root 的远端 `anterm-server` 一键部署。
- **内置代码编辑器与 Git SCM**：支持停靠/全屏代码编辑、LSP 补全/跳转/诊断、`syntect` 语法高亮、并排/统一 Diff 查看器以及窗口化 Git 提交图。

## 核心功能一览

| 模块 | 主要能力 |
| --- | --- |
| **命令面板** | <kbd>⌘ P</kbd> / <kbd>Ctrl+Shift+P</kbd> Zed 风格纯命令面板 · 按使用频率自动置顶 · 内置 **切换标签页（Switch Tab）**、**切换标签（Switch Tag）**、**切换主题**、**转到文件**、**转到符号**、**SSH 主机**、**Shell** 等独立子选择器 |
| **标签页、标签与分屏** | 四向分屏（**向右 / 向左 / 向下 / 向上**）· 自定义 **Tag**（语义色点、侧边栏筛选栏、分组标题、<kbd>⌘ ⇧ L</kbd> 编辑标签）· 休眠标签页释放内存 |
| **Agent 感知** | 逐 pane 识别 28 款 CLI agent · 一键安装 hook 获得状态点、系统通知、托盘提醒、重启续接与运行中会话 Fork |
| **提示符与输入体验** | 历史内联建议 · Tab 补全说明 · 多行编辑与 Composer（<kbd>⌘ I</kbd>）· 可选 **Helix** 模态编辑 · 点击定位光标 · <kbd>⌃ R</kbd> 模糊历史搜索 |
| **侧边面板与 Git** | <kbd>⌘ J</kbd> / <kbd>Ctrl+Shift+J</kbd> · 虚拟化文件树 · 源代码管理改动、Diff 与提交图 · 文件内容搜索 · 实时进程树与监听端口 |
| **内置代码编辑器** | 停靠列或全屏视图 · 编辑器分屏 · LSP 语言服务 · Git 行首改动标记与并排/统一 Diff · Markdown 预览 |
| **SSH 与 SFTP** | 自带 `russh` 协议栈（不依赖外部 `ssh`）· 主机配置独立保存在 `servers.json`，密钥存入系统钥匙串 · SFTP 面板 · 端口转发 · 跳板机 |
| **Shell 集成** | pane 启动时自动注入，零安装成本 · 提示符边界、工作目录、退出码与长耗时命令通知 · 覆盖 zsh、bash、fish、PowerShell、WSL 与远端 pane |
| **国际化与热重载** | 内置中英文界面（`en` / `zh-CN`）、完整输入法支持与 `config.json` 修改实时热重载 |

## 安装

### 预编译发布包

前往 [**GitHub Releases**](https://github.com/netkn001/anterm/releases) 下载对应平台的原生构建：

| 平台 | 安装包 | 说明 |
| --- | --- | --- |
| **macOS** | `anterm-…-macos-arm64.dmg` · `…-macos-x86_64.dmg` | 拖入「应用程序」（macOS 12.0+） |
| **Windows** | `anterm-…-setup.exe` · 免安装 `….zip` | 静态链接 MSVC CRT；内置 Windows ConPTY 运行时 |
| **Linux** | `anterm-…-x86_64.AppImage` / `.tar.gz` | 独立桌面端与无头 `anterm-server` 二进制 |

### 从源码构建

需要 **Rust 1.85+**（Rust 2024 edition）及对应平台的 GUI 编译依赖（详见 [CONTRIBUTING.md](CONTRIBUTING.md#development-prerequisites)）。

```bash
# 开发模式运行（自动使用隔离的 .anterm-dev/ 配置目录，不影响日常配置）
cargo dev

# 构建 release 二进制（anterm-app、anterm-server、anterm-updater）并打包到 dist/
./scripts/package-release.sh          # macOS .app/.zip 或 Linux .tar.gz + SHA256SUMS.txt
./scripts/package-release.sh --dmg    # (macOS) 额外生成可拖拽安装的 .dmg 镜像
.\scripts\package-windows.ps1         # (Windows PowerShell) 便携版 .zip + anterm-server.exe + checksums.txt
```

仅编译无头会话服务端（零 GUI、SQLite、钥匙串依赖）：

```bash
cargo build --release -p anterm-server
```

## 配置与快捷键

所有应用配置保存在单个支持实时热重载的 JSON 文件中（SSH 主机列表独立存放在 `servers.json`，方便将 `config.json` 纳入 dotfiles 同步）：

- **macOS / Linux**：`~/.config/anterm/config.json`
- **Windows**：`%APPDATA%\anterm\config.json`（绿色版可放置 `.anterm-portable` 并创建 `data/` 目录启用便携数据）
- **临时覆盖**：命令行 `--config-dir <path>` 或环境变量 `ANTERM_CONFIG_DIR`

完整参考文档：

- [**配置与命令行参考（Configuration & CLI）**](docs/CONFIGURATION.md) — 配置路径优先级、`config.json` 字段详解、Agent / Shell / 代理配置以及 `anterm-app` / `anterm-server` CLI 参数
- [**快捷键参考（Keybindings）**](docs/KEYBINDINGS.md) — macOS / Windows / Linux 默认快捷键总表及自定义覆盖语法

## 支持的 Coding Agent

**识别**开箱即用：自动展示品牌头像、Git 分支与 diff、标签页标题。
**状态与恢复**只需在 **设置 → 集成** 中一键安装对应 hook，即可获得状态点、系统通知、托盘提醒与重启后会话恢复。
**Fork** 适用于同时支持会话 ID hook 与原生 fork 命令的 agent。

<details>
<summary>28 款 Coding Agent 完整支持矩阵</summary>

| Agent | 识别 | 状态 · 重启恢复 | Fork |
| --- | :---: | :---: | :---: |
| **Claude Code** | ✓ | ✓ | ✓ |
| **Codex** | ✓ | ✓ | ✓ |
| **TraeCode** | ✓ | ✓ | ✓ |
| **Grok** | ✓ | ✓ | ✓ |
| **OpenCode** | ✓ | ✓ | ✓ |
| **Oh My Pi** | ✓ | ✓ | ✓ |
| **Prime Agent** | ✓ | ✓ | ✓ |
| **Droid** | ✓ | ✓ | ✓ |
| **Qwen Code** | ✓ | ✓ | ✓ |
| **Goose** | ✓ | ✓ | ✓ |
| **Qoder CLI** | ✓ | ✓ | ✓ |
| **Qoder CN CLI** | ✓ | ✓ | ✓ |
| **CodeBuddy** | ✓ | ✓ | ✓ |
| **Gemini** | ✓ | ✓ | |
| **Copilot** | ✓ | ✓ | |
| **Kimi Code** | ✓ | ✓ | |
| **Pi** | ✓ | ✓ | |
| **Crush** | ✓ | ✓ | |
| **Antigravity** | ✓ | ✓ | |
| **Cursor** | ✓ | ✓ | |
| **Empryo** | ✓ | ✓ | |
| **Muse Code** | ✓ | ✓ | |
| **jcode** | ✓ | ✓ | |
| Aider | ✓ | | |
| Amp | ✓ | | |
| Auggie | ✓ | | |
| Hermes | ✓ | | |
| Vibe | ✓ | | |

</details>

anterm 不包装、不代理其中任何一个 agent —— 你启动的就是原版程序本身，运行在标准 PTY 中。若使用自定义 wrapper 脚本启动，可通过 `config.json` 中的 `agent_commands` 映射识别，并通过 `agent_launch` 设置默认启动参数。

## 架构概览

项目采用 **13-crate 严格无环（Acyclic DAG）Cargo 虚拟工作区**：

- **无头核心与工具层（零 GPUI 依赖）**：`anterm-protocol`、`anterm-i18n`、`anterm-editor`、`anterm-core`、`anterm-server`、`anterm-updater`
- **桌面端 GPUI UI 层**：`anterm-ui-kit`、`anterm-terminal`、`anterm-remote-ui`、`anterm-files-ui`、`anterm-editor-ui`、`anterm-settings-ui`、`anterm-app`

详细依赖关系图、各 crate 职责与静态门禁命令请参阅 [架构指南（Architecture Guide）](docs/ARCHITECTURE.md)。

## 文档与社区

- [架构指南](docs/ARCHITECTURE.md) — 13-crate 工作区划分、依赖 DAG、模块边界与验证命令
- [配置与命令行参考](docs/CONFIGURATION.md) — `config.json`、`servers.json`、环境变量与 CLI 参数
- [快捷键参考](docs/KEYBINDINGS.md) — 跨平台默认快捷键与自定义覆盖方法
- [贡献指南](CONTRIBUTING.md) — 编译环境依赖、本地开发流程与 PR 检查清单
- [更新日志](CHANGELOG.md) · [安全策略](SECURITY.md) · [隐私政策](PRIVACY.md) · [行为准则](CODE_OF_CONDUCT.md) · [第三方开源声明](THIRD_PARTY_NOTICES.md)

## 开源协议

Copyright (c) 2026 anterm contributors.

本项目基于 [MIT License](LICENSE) 开源。内置字体、运行时组件与核心第三方依赖的许可信息请参阅 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
