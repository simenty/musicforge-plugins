//! `lyrics-online` 插件入口：LRCLIB 歌词匹配核验（X8 NDJSON 服务环）。

fn main() {
    musicforge_plugins::serve(
        "lyrics-online",
        &["title", "artists", "duration_ms", "lyrics_excerpt"],
        musicforge_plugins::lyrics_lrclib::handler,
    );
}
