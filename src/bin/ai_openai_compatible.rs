//! `ai-openai-compatible` 插件入口：X8 NDJSON 服务环。
//!
//! 能力域：LLM（identify/lyrics.verify/cover.generate）+ 确定性本地
//! （filename_regex/review_duplicate_group）。环境变量见 README。

fn main() {
    musicforge_plugins::serve(
        "ai-openai-compatible",
        &[
            "normalized_filename",
            "title",
            "artists",
            "album",
            "duration_ms",
            "format",
            "language_hint",
            "lyrics_excerpt",
        ],
        musicforge_plugins::ai_openai::handler,
    );
}
