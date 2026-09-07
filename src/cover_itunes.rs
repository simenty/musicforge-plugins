//! `cover-online`：iTunes Search API 的封面候选（`cover.search`）。
//!
//! D22：封面来源优先级最末——本插件只应在 core 判定「无达标内嵌」后发起；
//! 请求负载只含 title/artists/album（无绝对路径、无字节）。

use musicforge_plugin_api::{CoverCandidate, CoverResult};

use crate::{http_get_json, unknown_method};

pub fn handler(method: &str, params: serde_json::Value) -> Result<serde_json::Value, (String, String)> {
    match method {
        musicforge_plugin_api::methods::COVER_SEARCH => {
            search(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        other => Err(unknown_method(other)),
    }
}

fn base_url() -> String {
    std::env::var("ITUNES_BASE").unwrap_or_else(|_| "https://itunes.apple.com".into())
}

/// 店面兜底链（D23 中文优先）。`ITUNES_COUNTRY` 支持逗号分隔多店面。
///
/// 真机实测（2026-09-08）：iTunes Search API 的 cn 店面对中文音乐目录
/// 普遍返回 0 结果（Apple 目录限制，非本插件问题），tw/us 正常——
/// 默认链 `cn,tw,us`：先中文目录，逐店兜底直到命中。
fn countries() -> Vec<String> {
    std::env::var("ITUNES_COUNTRY")
        .unwrap_or_else(|_| "cn,tw,us".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 构造搜索 URL（纯函数；term = 艺人 + 专辑/标题，仅已声明字段）。
pub fn search_url(title: &str, artists: &[String], album: Option<&str>, country: &str) -> String {
    let mut parts: Vec<String> = artists.to_vec();
    if let Some(al) = album {
        parts.push(al.to_string());
    }
    parts.push(title.to_string());
    let term = parts.join(" ");
    format!(
        "{}/search?term={}&entity=album&limit=5&country={}",
        base_url(),
        urlencode(&term),
        country
    )
}

/// artworkUrl100 → 600x600 升采样 URL（600 ≥ 500px 质量门限，D22）。
pub fn upscale_artwork(url_100: &str) -> String {
    url_100.replace("100x100bb.jpg", "600x600bb.jpg")
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

/// `cover.search`：iTunes 专辑搜索 → 封面候选（按名次衰减置信度）。
///
/// 查询策略（两级 × 多店面兜底链）：
/// 1. 主查询：有专辑名 → `艺人 + 专辑`；无专辑 → `艺人 + 标题`
///    （专辑搜索对「歌名≠专辑名」的曲库命中率低——真机实测：晴天(歌)≠叶惠美(专辑)）；
/// 2. 主查询 0 结果 → `艺人` 单词检索；
/// 3. 仍 0 → 下一店面（cn→tw→us），直到命中或店面链耗尽。
pub fn search(params: &serde_json::Value) -> Result<CoverResult, (String, String)> {
    let p: musicforge_plugin_api::CoverQueryParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    for country in countries() {
        let url = search_url(&p.title, &p.artists, p.album.as_deref(), &country);
        let v = http_get_json(&url).map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
        let result = candidates_from_results(&v);
        if !result.candidates.is_empty() {
            return Ok(result);
        }
        if p.artists.is_empty() {
            continue;
        }
        // 兜底：仅按艺人检索（取其专辑封面集）
        let fallback_url = format!(
            "{}/search?term={}&entity=album&limit=5&country={}",
            base_url(),
            urlencode(p.artists.join(" ").as_str()),
            country
        );
        let v2 = http_get_json(&fallback_url).map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
        let fallback = candidates_from_results(&v2);
        if !fallback.candidates.is_empty() {
            return Ok(fallback);
        }
    }
    Ok(CoverResult::default()) // 全链 0 命中 → 空候选（诚实：不伪造封面）
}

/// 纯函数：iTunes 响应 → 候选列表（离线可测）。
pub fn candidates_from_results(results: &serde_json::Value) -> CoverResult {
    let arr = results["results"].as_array().cloned().unwrap_or_default();
    let candidates = arr
        .iter()
        .filter_map(|r| {
            let art = r["artworkUrl100"].as_str()?;
            let collection = r["collectionName"].as_str().unwrap_or("");
            Some(CoverCandidate {
                source: format!("itunes/{collection}"),
                image_ref: upscale_artwork(art),
                width_px: Some(600),
                height_px: Some(600),
                confidence: 0.0, // 下面按名次覆盖
            })
        })
        .enumerate()
        .map(|(i, mut c)| {
            // 名次衰减：0.85 起步，每名 -0.1，下限 0.45（仍是可用候选）
            c.confidence = (0.85 - 0.1 * i as f32).max(0.45);
            c
        })
        .collect();
    CoverResult { candidates }
}
