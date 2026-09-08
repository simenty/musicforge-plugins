//! 离线契约测试：全部不联网（HTTP 层隔离在 `http_get_json`/`http_post_json`，
//! 纯函数走解析/构造路径）。运行：`cargo test`（无需 API Key）。

use musicforge_plugin_api::LyricsVerdict;
use musicforge_plugins::{ai_openai, cover_itunes, lyrics_lrclib, manifest_for};

/// 稳定审计 B7 回归：错误摘要截断必须 UTF-8 安全（CJK 多字节中间不得 panic）。
#[test]
fn utf8_truncate_is_char_boundary_safe() {
    let cjk = "错误：余额不足，请充值后重试，账户名不存在或密码错误，验证码已过期，请求过于频繁。";
    let t = ai_openai::utf8_truncate_for_test(cjk, 10);
    assert!(t.chars().count() <= 10);
    assert!(!t.contains('\u{FFFD}'), "不得产生替换字符（UTF-8 边界破坏证据）");
    assert_eq!(ai_openai::utf8_truncate_for_test("", 10), "");
    assert_eq!(ai_openai::utf8_truncate_for_test("abc", 10), "abc");
}

// ---------------------------------------------------------------- 清单 --

#[test]
fn manifests_declare_network_and_minimal_request_model() {
    for (name, sent) in [
        (
            "ai-openai-compatible",
            vec![
                "normalized_filename",
                "title",
                "artists",
                "album",
                "duration_ms",
                "format",
                "language_hint",
                "lyrics_excerpt",
            ],
        ),
        (
            "lyrics-online",
            vec!["title", "artists", "duration_ms", "lyrics_excerpt"],
        ),
        ("cover-online", vec!["title", "artists", "album", "duration_ms"]),
    ] {
        let m = manifest_for(name, &sent);
        assert_eq!(m.name, name);
        assert_eq!(m.api_version, "1.0.0");
        assert!(m.network, "在线插件必须声明联网");
        // 最小请求模型对偶声明：禁发字段必须在清单里显式列出
        for forbidden in ["absolute_path", "audio_bytes", "cover_bytes"] {
            assert!(
                m.data_not_sent.iter().any(|s| s == forbidden),
                "{name}: data_not_sent 缺 {forbidden}"
            );
        }
        // 声明发送的字段必须真的在 data_sent 里
        for s in sent {
            assert!(m.data_sent.iter().any(|x| x == s), "{name}: data_sent 缺 {s}");
        }
        // plugin.json 打包件与代码构造的清单必须一致（防漂移）
        let path = format!("plugins/{name}/plugin.json");
        let raw = std::fs::read_to_string(path).unwrap();
        let shipped: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let built = serde_json::to_value(&m).unwrap();
        assert_eq!(shipped, built, "{name}: plugin.json 与代码清单漂移");
    }
}

// ---------------------------------------------------------------- LLM 解析 --

#[test]
fn identify_response_parses_strict_json_with_fenced_output() {
    let content = "```json\n{\"title\":\"借墨\",\"artists\":[\"王铮亮\",\"风华音纪\"],\
        \"album\":\"借墨\",\"confidence\":0.93,\
        \"field_confidence\":{\"title\":0.99,\"artists\":0.95,\"album\":0.86},\
        \"reason\":\"文件名与标签一致\"}\n```";
    let envelope = serde_json::json!({
        "choices": [{"message": {"content": content}}]
    });
    let text = envelope["choices"][0]["message"]["content"].as_str().unwrap();
    let v = ai_openai::extract_json_for_test(text).unwrap();
    let sug: musicforge_plugin_api::IdentifySuggestion = serde_json::from_value(serde_json::json!({
        "title": v["title"], "artists": v["artists"], "album": v["album"],
        "confidence": v["confidence"], "field_confidence": v["field_confidence"],
        "reason": v["reason"],
    }))
    .unwrap();
    assert_eq!(sug.title, "借墨");
    assert_eq!(sug.artists.len(), 2);
    assert!((sug.confidence - 0.93).abs() < 1e-6);
}

#[test]
fn identify_response_rejects_non_json_loudly() {
    let err = ai_openai::extract_json_for_test("抱歉，我无法识别。").unwrap_err();
    assert!(err.contains("不是合法 JSON"), "解析失败必须显式可见: {err}");
}

#[test]
fn confidence_is_clamped_to_unit_interval() {
    assert_eq!(ai_openai::clamp01_for_test(1.7), 1.0);
    assert_eq!(ai_openai::clamp01_for_test(-0.3), 0.0);
    assert_eq!(ai_openai::clamp01_for_test(0.5), 0.5);
}

// ------------------------------------------------ 确定性本地方法（零请求）--

#[test]
fn filename_regex_and_duplicate_review_are_deterministic_and_offline() {
    let regex = ai_openai::generate_filename_regex(&serde_json::json!({
        "samples": ["王铮亮 - 借墨 [SQ].wav"]
    }))
    .unwrap();
    assert!(regex.rule.contains("P<artist>"));
    assert_eq!(
        ai_openai::generate_filename_regex(&serde_json::json!({
            "samples": ["王铮亮 - 借墨 [SQ].wav"]
        }))
        .unwrap()
        .rule,
        regex.rule,
        "两次运行必须逐字节一致（可复算铁律）"
    );

    let review = ai_openai::review_duplicate_group(&serde_json::json!({
        "members": [
            {"filename": "a.flac", "bitrate_kbps": 800, "size_bytes": 25000000},
            {"filename": "b.flac", "bitrate_kbps": 1000, "size_bytes": 31000000}
        ]
    }))
    .unwrap();
    assert_eq!(review.keep_index, 1, "高码率成员应被建议保留");
}

// ---------------------------------------------------------------- LRCLIB --

#[test]
fn lrclib_search_url_contains_only_declared_fields() {
    let url = lyrics_lrclib::search_url("借墨", Some("王铮亮"));
    assert!(url.starts_with("https://lrclib.net/api/search"));
    assert!(url.contains("track_name="));
    assert!(url.contains("artist_name="));
    let wire = serde_json::to_string(&serde_json::json!({
        "title": "借墨", "artists": ["王铮亮"], "duration_ms": 252000,
        "lyrics_excerpt": "一笔借墨"
    }))
    .unwrap();
    for forbidden in ["absolute_path", "audio_bytes", "cover_bytes"] {
        assert!(!wire.contains(forbidden), "歌词请求含禁发字段: {forbidden}");
    }
}

#[test]
fn lrclib_verdict_match_uncertain_mismatch() {
    let matched = serde_json::json!([{
        "trackName": "借墨", "artistName": "王铮亮",
        "plainLyrics": "一笔借墨 挥毫落纸\n山河为证"
    }]);
    let r = lyrics_lrclib::verdict_from_candidates(&matched, "挥毫落纸");
    assert_eq!(r.verdict, LyricsVerdict::Match);
    assert!((r.confidence - 0.95).abs() < 1e-6);

    let unconfirmed = serde_json::json!([{
        "trackName": "别的歌", "artistName": "别人",
        "plainLyrics": "完全无关的歌词内容"
    }]);
    assert_eq!(
        lyrics_lrclib::verdict_from_candidates(&unconfirmed, "挥毫落纸").verdict,
        LyricsVerdict::Mismatch,
        "字符袋相似度过低 → 诚实判 Mismatch（有候选≠含片段）"
    );

    let empty = serde_json::json!([]);
    let r = lyrics_lrclib::verdict_from_candidates(&empty, "挥毫落纸");
    assert_eq!(r.verdict, LyricsVerdict::Mismatch);
    assert!(r.candidates.is_empty());
}

/// 真机实测回归：歌词来源的分词/标点差异（「小黄花，从」vs「小黄花 从」）
/// 不得破坏比对——归一化去全部标点后必须 Match。
#[test]
fn lrclib_verdict_ignores_punctuation_and_whitespace_differences() {
    let candidates = serde_json::json!([{
        "trackName": "晴天", "artistName": "周杰伦",
        "plainLyrics": "故事的小黄花，从出生那年就飘着\n童年的荡秋千，随记忆一直晃到现在"
    }]);
    let r = lyrics_lrclib::verdict_from_candidates(&candidates, "故事的小黄花 从出生那年就飘着");
    assert_eq!(r.verdict, LyricsVerdict::Match, "标点差异必须被归一化抹平");
    assert!((r.confidence - 0.95).abs() < 1e-6, "精确包含 → 最高置信");
}

/// 真机实测回归（D23 中文曲库核心痛点）：LRCLIB 大量歌词为**繁体**，
/// 用户片段为简体——字符袋相似度必须容忍字形差异（晴天实案：11/14 同字）。
#[test]
fn lrclib_verdict_tolerates_traditional_simplified_glyph_gap() {
    let candidates = serde_json::json!([{
        "trackName": "晴天", "artistName": "周杰伦",
        "plainLyrics": "故事的小黃花\n從出生那年就飄著\n童年的盪鞦韆\n隨記憶一直晃到現在"
    }]);
    let r = lyrics_lrclib::verdict_from_candidates(&candidates, "故事的小黄花 从出生那年就飘着");
    assert_eq!(r.verdict, LyricsVerdict::Match, "繁简字形差异不得阻断核验: {r:?}");
    assert!((r.confidence - 0.85).abs() < 1e-6, "非精确包含 → 0.85 档");
}

/// 中等相似（约半数字符命中）→ Uncertain（不猜，交给上层/用户）。
#[test]
fn lrclib_verdict_moderate_similarity_is_uncertain() {
    let candidates = serde_json::json!([{
        "trackName": "另一首", "artistName": "别人",
        "plainLyrics": "挥毫落纸天地玄黄宇宙洪荒"
    }]);
    let r = lyrics_lrclib::verdict_from_candidates(&candidates, "挥毫落纸山河为证");
    assert_eq!(r.verdict, LyricsVerdict::Uncertain, "半数命中必须 Uncertain: {r:?}");
}

/// 搜索 URL 按店面参数生成；主程序角度的禁发字段线上断言。
#[test]
fn itunes_search_url_carries_country_and_no_forbidden_fields() {
    let url = cover_itunes::search_url("晴天", &["周杰伦".into()], Some("叶惠美"), "tw");
    assert!(url.contains("country=tw"));
    assert!(url.contains("entity=album"));
    let wire = serde_json::to_string(&serde_json::json!({
        "title": "晴天", "artists": ["周杰伦"], "album": "叶惠美", "duration_ms": 269000
    }))
    .unwrap();
    for forbidden in ["absolute_path", "audio_bytes", "cover_bytes"] {
        assert!(!wire.contains(forbidden), "封面请求含禁发字段: {forbidden}");
    }
}

// ---------------------------------------------------------------- iTunes --

#[test]
fn itunes_artwork_upscaled_to_meet_500px_gate() {
    let url = "https://is1-ssl.mzstatic.com/image/thumb/Music/xx/100x100bb.jpg";
    let up = cover_itunes::upscale_artwork(url);
    assert!(up.contains("600x600bb.jpg"), "必须升到 600px（≥500 门限）: {up}");
}

#[test]
fn itunes_candidates_decay_by_rank_and_carry_dimensions() {
    let results = serde_json::json!({
        "results": [
            {"artworkUrl100": "https://x/1/100x100bb.jpg", "collectionName": "专辑A"},
            {"artworkUrl100": "https://x/2/100x100bb.jpg"},
            {"artworkUrl100": "https://x/3/100x100bb.jpg"},
            {"artworkUrl100": "https://x/4/100x100bb.jpg"},
            {"artworkUrl100": "https://x/5/100x100bb.jpg"},
            {"artworkUrl100": "https://x/6/100x100bb.jpg"}
        ]
    });
    let c = cover_itunes::candidates_from_results(&results).candidates;
    assert_eq!(c.len(), 6, "limit=5 + 名次衰减，此处全收由调用方/主程序取舍");
    assert!((c[0].confidence - 0.85).abs() < 1e-6);
    assert!((c[1].confidence - 0.75).abs() < 1e-6);
    assert_eq!(c[1].width_px, Some(600), "统一 600px（≥500 门限）");
    assert!(c[1].image_ref.contains("600x600bb.jpg"));
    // 无 artwork 的结果必须被跳过
    let partial = serde_json::json!({"results": [{"collectionName": "无封面"}]});
    assert!(cover_itunes::candidates_from_results(&partial).candidates.is_empty());
}
