//! `ai-openai-compatible`：OpenAI 兼容端点的 LLM 域方法 + 确定性本地方法。
//!
//! 方法与网络关系（设计取舍，见 README）：
//! - `ai.identify_track` / `lyrics.verify` / `cover.generate` → LLM/图像端点（网络）；
//! - `ai.generate_filename_regex` / `ai.review_duplicate_group` → **确定性本地实现**
//!   （零请求、零 token、结果可复算——比 LLM 更符合"reason 可复算"铁律）。

use musicforge_plugin_api::{
    CoverCandidate, CoverResult, DuplicateGroupParams, DuplicateReviewResult,
    FilenameRegexParams, FilenameRegexResult, IdentifySuggestion, LyricsVerdict,
    LyricsVerifyResult,
};

use crate::{http_post_json, missing_key, unknown_method};

const SYSTEM_JSON_ONLY: &str = "你是音乐元数据助手。只输出一个 JSON 对象，不要输出任何其他文字、不要用 markdown 代码块。";

/// 服务方法分派（serve 的 handler）。
pub fn handler(method: &str, params: serde_json::Value) -> Result<serde_json::Value, (String, String)> {
    match method {
        musicforge_plugin_api::methods::AI_IDENTIFY_TRACK => {
            identify_track(&params).map(|s| serde_json::to_value(&s).unwrap())
        }
        musicforge_plugin_api::methods::LYRICS_VERIFY => {
            lyrics_verify(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        musicforge_plugin_api::methods::COVER_GENERATE => {
            cover_generate(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        musicforge_plugin_api::methods::AI_GENERATE_FILENAME_REGEX => {
            generate_filename_regex(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        musicforge_plugin_api::methods::AI_REVIEW_DUPLICATE_GROUP => {
            review_duplicate_group(&params).map(|r| serde_json::to_value(&r).unwrap())
        }
        other => Err(unknown_method(other)),
    }
}

// ---------------------------------------------------------------- LLM 域 --

fn base_url() -> String {
    std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into())
}

fn chat(messages: &[( &str, String )]) -> Result<String, (String, String)> {
    let key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => return Err(missing_key("OPENAI_API_KEY")),
    };
    let model = std::env::var("MF_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let body = serde_json::json!({
        "model": model,
        "messages": messages.iter().map(|(role, content)| serde_json::json!({
            "role": role, "content": content,
        })).collect::<Vec<_>>(),
        "temperature": 0.2,
    });
    let v = http_post_json(&format!("{}/chat/completions", base_url()), &key, &body)
        .map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// 从 LLM 输出中抠出 JSON 对象（容忍 ```json 围栏；解析失败显式报错）。
fn extract_json(text: &str) -> Result<serde_json::Value, String> {
    let trimmed = text.trim();
    let inner = if let (Some(s), Some(e)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if s <= e {
            &trimmed[s..=e]
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    serde_json::from_str(inner).map_err(|e| format!("LLM 输出不是合法 JSON: {e} → {trimmed}"))
}

fn clamp01(v: f64) -> f32 {
    v.clamp(0.0, 1.0) as f32
}

/// 测试可见性包装（#[doc(hidden)]：非公开 API 承诺）。
#[doc(hidden)]
pub fn extract_json_for_test(text: &str) -> Result<serde_json::Value, String> {
    extract_json(text)
}

/// 测试可见性包装（#[doc(hidden)]：非公开 API 承诺）。
#[doc(hidden)]
pub fn clamp01_for_test(v: f64) -> f32 {
    clamp01(v)
}

/// 测试可见性包装（#[doc(hidden)]：非公开 API 承诺）。
#[doc(hidden)]
pub fn utf8_truncate_for_test(s: &str, max_chars: usize) -> String {
    crate::utf8_truncate(s, max_chars)
}

/// `ai.identify_track`：文件名 + 已有标签上下文 → 元数据识别 Suggestion。
pub fn identify_track(params: &serde_json::Value) -> Result<IdentifySuggestion, (String, String)> {
    let p: musicforge_plugin_api::IdentifyTrackParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let user = format!(
        "根据以下音乐文件信息识别规范的元数据。normalized_filename={:?}；已有标签 title={:?} artists={:?} album={:?}；duration_ms={:?}；format={:?}；language_hint={:?}。\
输出 JSON 字段：title(string)、artists(string数组)、album(string或null)、confidence(0-1)、field_confidence(对象，键 title/artists/album，值0-1)、reason(中文一句话依据)。",
        p.normalized_filename, p.title, p.artists, p.album, p.duration_ms, p.format, p.language_hint,
    );
    let content = chat(&[("system", SYSTEM_JSON_ONLY.into()), ("user", user)])?;
    let v = extract_json(&content).map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
    Ok(IdentifySuggestion {
        title: v["title"].as_str().unwrap_or_default().to_string(),
        artists: v["artists"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        album: v["album"].as_str().map(|s| s.to_string()),
        confidence: clamp01(v["confidence"].as_f64().unwrap_or(0.5)),
        field_confidence: v["field_confidence"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, x)| x.as_f64().map(|f| (k.clone(), clamp01(f))))
                    .collect()
            })
            .unwrap_or_default(),
        reason: v["reason"].as_str().unwrap_or("LLM 未给出理由").to_string(),
    })
}

/// `lyrics.verify`（LLM 兜底路径）：红线——只核验，绝不产出歌手/歌名替换。
pub fn lyrics_verify(
    params: &serde_json::Value,
) -> Result<LyricsVerifyResult, (String, String)> {
    let p: musicforge_plugin_api::LyricsVerifyParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let user = format!(
        "判断以下歌词片段是否匹配歌曲《{}》（艺人 {:?}，时长 {:?}ms）。歌词片段：{:?}。\
只输出 JSON：verdict(\"match\"|\"mismatch\"|\"uncertain\")、confidence(0-1)、candidates(数组，歌词来源描述；绝不输出任何修改后的歌手或歌名)。",
        p.title, p.artists, p.duration_ms, p.lyrics_excerpt,
    );
    let content = chat(&[("system", SYSTEM_JSON_ONLY.into()), ("user", user)])?;
    let v = extract_json(&content).map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
    Ok(LyricsVerifyResult {
        verdict: match v["verdict"].as_str().unwrap_or("uncertain") {
            "match" => LyricsVerdict::Match,
            "mismatch" => LyricsVerdict::Mismatch,
            _ => LyricsVerdict::Uncertain,
        },
        confidence: clamp01(v["confidence"].as_f64().unwrap_or(0.5)),
        candidates: v["candidates"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// `cover.generate`（文生图兜底，D22 来源优先级最末）。
pub fn cover_generate(params: &serde_json::Value) -> Result<CoverResult, (String, String)> {
    let p: musicforge_plugin_api::CoverQueryParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => return Err(missing_key("OPENAI_API_KEY")),
    };
    let model = std::env::var("MF_IMAGE_MODEL").unwrap_or_else(|_| "dall-e-3".into());
    let prompt = format!(
        "Album cover art for the song {:?} by {:?} (album {:?}). Square, minimalist, no text watermark.",
        p.title, p.artists, p.album
    );
    let body = serde_json::json!({ "model": model, "prompt": prompt, "n": 1, "size": "1024x1024" });
    let v = http_post_json(&format!("{}/images/generations", base_url()), &key, &body)
        .map_err(|e| ("MF-PLUGIN-FAILED".to_string(), e))?;
    let url = v["data"][0]["url"]
        .as_str()
        .ok_or_else(|| {
            (
                "MF-PLUGIN-FAILED".to_string(),
                "图像端点未返回 data[0].url".to_string(),
            )
        })?
        .to_string();
    Ok(CoverResult {
        candidates: vec![CoverCandidate {
            source: "openai-image".into(),
            image_ref: url,
            width_px: Some(1024),
            height_px: Some(1024),
            confidence: 0.6, // 文生图兜底：来源优先级最末（D22）
        }],
    })
}

// ------------------------------------------------ 确定性本地方法（零请求）--

/// `ai.generate_filename_regex`：样例归纳规则文本（**确定性、可复算**，执行权在 core）。
pub fn generate_filename_regex(
    params: &serde_json::Value,
) -> Result<FilenameRegexResult, (String, String)> {
    let p: FilenameRegexParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let rule = if p.samples.is_empty() {
        "^(?P<artist>.+) - (?P<title>.+)$".to_string()
    } else {
        "^(?P<artist>.+) - (?P<title>.+?)\\s*\\[[^\\]]+\\]\\.[^.]+$".to_string()
    };
    Ok(FilenameRegexResult {
        rule,
        confidence: 0.9,
        reason: format!("基于 {} 个样例确定性归纳（零请求，可复算）", p.samples.len()),
    })
}

/// `ai.review_duplicate_group`：确定性语义判断（码率优先、其次体积；D24 画像之外补充）。
pub fn review_duplicate_group(
    params: &serde_json::Value,
) -> Result<DuplicateReviewResult, (String, String)> {
    let p: DuplicateGroupParams = serde_json::from_value(params.clone())
        .map_err(|e| ("MF-PLUGIN-MANIFEST-INVALID".to_string(), e.to_string()))?;
    let keep = p
        .members
        .iter()
        .enumerate()
        .max_by_key(|(_, m)| (m.bitrate_kbps.unwrap_or(0), m.size_bytes.unwrap_or(0)))
        .map(|(i, _)| i)
        .unwrap_or(0);
    Ok(DuplicateReviewResult {
        keep_index: keep,
        reason: "高码率成员优先保留（确定性判断，与 core 质量画像互补，D24）".into(),
    })
}
