# musicforge-plugins — MusicForge 首发插件（AI + 在线提供方）

MusicForge 主程序（[simenty/MusicForge](https://github.com/simenty/MusicForge)）
的**独立插件仓**：独立版本、独立许可证（Apache-2.0；主程序为 MIT）、**默认禁用**。

## 插件清单（v0.7.0 首发，D23 顺序）

| 插件 | 二进制 | 服务方法 | 数据源 | 网络 |
|:--|:--|:--|:--|:--|
| `ai-openai-compatible` | `ai-openai-compatible` | `ai.identify_track` / `lyrics.verify` / `cover.generate`（LLM 域）+ `ai.generate_filename_regex` / `ai.review_duplicate_group`（**确定性本地实现，零请求**） | 任意 OpenAI 兼容端点 | ✓ |
| `lyrics-online` | `lyrics-online` | `lyrics.verify` | [LRCLIB](https://lrclib.net) 公开 API | ✓ |
| `cover-online` | `cover-online` | `cover.search` | iTunes Search API（默认 cn 店面，D23 中文优先） | ✓ |

三个插件共同实现 `plugin.manifest / plugin.health / plugin.shutdown` 生命周期方法
（X8 NDJSON 协议；协议 crate 与主仓共享）。

## 安装与启用

1. 构建：`cargo build --release`
2. 按插件复制到白名单目录（任意其他路径会被主程序拒绝）：
   - Windows：`%LOCALAPPDATA%\MusicForge\plugins\<插件名>\`
   - Linux/macOS：`~/.local/share/musicforge/plugins/<插件名>/`

   每个插件目录内放两样东西：
   - `plugin.json`（本仓 `plugins/<插件名>/plugin.json`）
   - 对应二进制（`target/release/<bin>`，Windows 带 `.exe`）
3. 启用：GUI「AI 与插件」面板勾选，或编辑 `config.json` 的
   `"plugins": {"enabled": ["ai-openai-compatible"]}`。**默认全禁**。

## 环境变量（按插件）

| 插件 | 变量 | 默认 |
|:--|:--|:--|
| ai-openai-compatible | `OPENAI_BASE_URL` | `https://api.openai.com/v1` |
| | `OPENAI_API_KEY` | （必填，否则 LLM 方法显式报错） |
| | `MF_AI_MODEL` | `gpt-4o-mini` |
| | `MF_IMAGE_MODEL` | `dall-e-3` |
| lyrics-online | `LRCLIB_BASE` | `https://lrclib.net` |
| cover-online | `ITUNES_BASE` | `https://itunes.apple.com` |
| | `ITUNES_COUNTRY` | `cn`（D23：中文曲库优先） |

插件内部请求超时 8s（早于 Host 的 10–30s 窗口，先自止而非被 kill）。

## 红线（与主仓 `docs/p6a-ai-interface.md` §4 一致）

1. **只建议，不执行**：一切产物是 Suggestion；写入必经 core 的
   用户确认 → Plan → Apply。本仓没有任何文件系统写入代码。
2. **最小请求模型**：`data_not_sent = ["absolute_path", "audio_bytes", "cover_bytes"]`；
   音频指纹本地算、只传哈希；请求负载只含已声明字段。
3. **降级完整性**：插件全禁/全挂时，主程序五域功能 100% 可用。

## 开发

```bash
cargo build            # 需要 git/网络拉取依赖（首次）
cargo test             # 全部为离线单测（HTTP 层隔离，不联网）
cargo build --release  # 发布产物
```

协议依赖：`musicforge-plugin-api`（path 依赖 `../MusicForge`，发布时锁定
git tag）。运行行为契约见主仓 `docs/p6a-ai-interface.md`（设计冻结）。

## 许可证

Apache-2.0（独立于主程序的 MIT）。各上游数据源（LRCLIB / iTunes Search /
OpenAI 兼容端点）的服務条款由用户自行遵守；本仓不提供任何绕过、破解或
批量下载能力（见主仓 `PLUGIN_POLICY.md` §4）。
