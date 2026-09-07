//! `lyrics-online`：LRCLIB 公开 API 的歌词匹配核验（`lyrics.verify`）。
//!
//! 红线：绝不改歌手/歌名——本方法只输出核验结论与歌词来源候选，
//! 结果类型（`LyricsVerifyResult`）在协议层就不存在标题/艺人替换字段。

use musicforge_plugin_api::{LyricsVerdict, LyricsVerifyResult};

use crate::{http_get_json, unknown_method};

pub fn handler(method: &str, params: serde_json::Value) -> Result<serde_json::Value, (String, String)> {
    match method {
        musicforge_plugin_api::methods::LYRICS_VERIFY => {
            verify(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        other => Err(unknown_method(other)),
    }
}

fn base_url() -> String {
    std::env::var("LRCLIB_BASE").unwrap_or_else(|_| "https://lrclib.net".into())
}

/// 构造 LRCLIB 搜索 URL（纯函数，测试用；仅含已声明字段 title/artists）。
pub fn search_url(title: &str, artist: Option<&str>) -> String {
    let mut url = format!("{}/api/search?track_name={}", base_url(), urlencode(title));
    if let Some(a) = artist {
        url.push_str(&format!("&artist_name={}", urlencode(a)));
    }
    url
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 文本归一化：只保留字母/数字/汉字（去空白与全部标点）。
///
/// 为什么不做空白折叠：真实歌词「故事的小黄花，从出生那年就飘着」的分词/
/// 标点因来源而异（LRC 逐行 vs 逐句），保留标点会让子串匹配失效；
/// 去掉全部非字母数字后，两端的分词差异被抹平，子串比对稳定。
pub fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// `lyrics.verify`：按 title+artist 搜候选 → 与片段归一化比对 → 结论。
pub fn verify(params: &serde_json::Value) -> Result<LyricsVerifyResult, (String, String)> {
    let p: musicforge_plugin_api::LyricsVerifyParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let artist = p.artists.first().map(|s| s.as_str());
    let url = search_url(&p.title, artist);
    let v = http_get_json(&url).map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
    Ok(verdict_from_candidates(&v, &p.lyrics_excerpt))
}

/// 字符袋相似度：两串归一化文本的多重集交集 / needle 长度（0–1）。
///
/// 为什么不用子串包含做唯一判据：中文曲库的真实痛点是**简繁字形差异**
/// （LRCLIB 大量繁体歌词 vs 用户提供简体片段）。维护完整简繁转换表过重，
/// 改用「字符袋」——差异字只占少数时相似度仍高，对字形差异天然鲁棒。
fn bag_similarity(needle: &str, haystack_window: &str) -> f64 {
    if needle.is_empty() || haystack_window.is_empty() {
        return 0.0;
    }
    let mut bag: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    for c in haystack_window.chars() {
        *bag.entry(c).or_default() += 1;
    }
    let mut hit = 0usize;
    let mut total = 0usize;
    for c in needle.chars() {
        total += 1;
        if let Some(n) = bag.get_mut(&c) {
            if *n > 0 {
                *n -= 1;
                hit += 1;
            }
        }
    }
    if total == 0 { 0.0 } else { hit as f64 / total as f64 }
}

/// 在 haystack 中取与 needle 等长的滑动窗口，返回最大字符袋相似度。
fn best_window_similarity(needle: &str, haystack: &str) -> f64 {
    let n: Vec<char> = needle.chars().collect();
    let h: Vec<char> = haystack.chars().collect();
    if n.is_empty() || h.len() < n.len() {
        return 0.0;
    }
    let window = h.len() - n.len();
    (0..=window)
        .map(|i| bag_similarity(needle, &h[i..i + n.len()].iter().collect::<String>()))
        .fold(0.0_f64, f64::max)
}

/// 纯函数：候选集 + 片段 → 核验结论（离线可测）。
///
/// 判定双指标：归一化精确包含 → Match（0.95）；否则滑动窗口字符袋相似度
/// ≥0.7 → Match（0.85，容忍简繁字形差异）；≥0.45 → Uncertain；否则 Mismatch。
pub fn verdict_from_candidates(candidates: &serde_json::Value, excerpt: &str) -> LyricsVerifyResult {
    let arr = candidates.as_array().cloned().unwrap_or_default();
    let needle = normalize(excerpt);
    let sources: Vec<String> = arr
        .iter()
        .filter_map(|c| {
            let plain = c["plainLyrics"].as_str()?;
            if normalize(plain).is_empty() {
                return None;
            }
            let name = c["trackName"].as_str().unwrap_or("lrclib");
            let artist = c["artistName"].as_str().unwrap_or("");
            Some(format!("lrclib/{artist}/{name}"))
        })
        .collect();
    let mut best = 0.0_f64;
    for c in &arr {
        if let Some(plain) = c["plainLyrics"].as_str() {
            let plain_norm = normalize(plain);
            if needle.is_empty() || plain_norm.is_empty() {
                continue;
            }
            let sim = if plain_norm.contains(&needle) {
                1.0
            } else {
                best_window_similarity(&needle, &plain_norm)
            };
            best = best.max(sim);
        }
    }
    let (verdict, confidence) = if best >= 0.7 {
        (LyricsVerdict::Match, if best >= 1.0 { 0.95 } else { 0.85 })
    } else if best >= 0.45 {
        (LyricsVerdict::Uncertain, 0.5)
    } else if sources.is_empty() {
        (LyricsVerdict::Mismatch, 0.7)
    } else {
        (LyricsVerdict::Mismatch, 0.6)
    };
    LyricsVerifyResult {
        verdict,
        confidence,
        candidates: sources,
    }
}
