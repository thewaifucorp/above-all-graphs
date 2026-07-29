# aag 快速上手（简体中文）

`aag` 是一个代码知识图谱，会把自己安装到本机的每一个编码 agent 里。单个 Rust
二进制文件：不需要 API key，不需要本地编译步骤，也不需要配置文件。

本文翻译自[英文快速上手](../README.md)。两者不一致时以英文版为准 —— 那份是跟
代码一起更新的。

## 安装

```bash
npm install -g @waifucorp/aag

# 带本地语义检索（体积更大，仍然是单个自包含文件）：
AAG_SEMANTIC=1 npm install -g @waifucorp/aag
```

postinstall 会下载对应平台的预编译二进制（Linux、macOS、Windows；x64 与
arm64），不编译任何东西。

从源码构建：

```bash
git clone https://github.com/thewaifucorp/above-all-graphs
cd above-all-graphs && cargo build --release   # 二进制位于 target/release/aag
```

## 使用

```bash
aag bigbang   # 每个仓库执行一次：建立索引，并接入本机所有 agent
aag ui        # 打开浏览器，所有仓库在同一个界面里
```

`bigbang` 一次做三件事：为仓库建索引、在 `.aag/` 生成完全离线的站点、把 `aag`
注册进检测到的每个 agent —— MCP 服务器、hooks、skills 和规则，各按该 agent 自
己的配置格式写入。整个过程幂等、只做增量、可完全撤销：`aag uninstall` 精确移
除写入过的内容。

## 图谱能回答的问题

```bash
aag explore "parser 是怎么解析 import 的"     # 某处如何工作，并附上源码
aag impact Graph                              # 改动它会波及什么
aag rename 旧名 新名 --write                  # 跨文件的协同重命名
git diff --name-only | aag affected --stdin   # 这次改动会影响哪些测试
aag areas                                     # 这个仓库由哪些区域组成
aag graph-diff main workspace                 # 当前分支对图谱做了什么
```

每条边都带着解析时的置信度：`EXTRACTED`（源码中明确写出）、`INFERRED`（启发式
推断）、`AMBIGUOUS`（无法确定）。采信 `AMBIGUOUS` 之前请先核对。

## 在 agent 里使用

执行 `bigbang` 之后，agent 里已经列出了 MCP 工具 `explore`，skills 也装好了。
没有需要配置的东西：直接用自然语言提问（“登录流程是怎么走的？”“改 `Store` 会
坏掉什么？”），agent 会去查图谱，而不是漫无目的地 grep。

索引会自己保持最新 —— 原生文件监听、每次 MCP 连接时对账、以及每次编辑后重新同
步的 hooks。没有需要你记住的“重新索引”命令。

## 延伸阅读

- [架构](architecture.md) —— 整条流水线如何工作
- [兼容性矩阵](compatibility.md) —— 语言、agent、平台
- [基准测试](benchmarks.md) —— 实测数字，以及已知的边界
- [迁移说明](migration.md) —— 版本之间会变什么
