//! `cover-online` 插件入口：iTunes 封面候选搜索（X8 NDJSON 服务环）。

fn main() {
    musicforge_plugins::serve(
        "cover-online",
        &["title", "artists", "album", "duration_ms"],
        musicforge_plugins::cover_itunes::handler,
    );
}
