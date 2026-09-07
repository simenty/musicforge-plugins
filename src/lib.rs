//! musicforge-plugins 首发插件公共层。
//!
//! 职责（对齐主仓 `docs/p6a-ai-interface.md` 冻结契约 + PLUGIN_POLICY.md）：
//! - **X8 NDJSON 服务环**：逐行读请求 → 分发 → 逐行写响应；生命周期方法
//!   （`plugin.manifest/health/shutdown`）在此统一实现，能力方法交给各插件；
//! - **最小请求模型**：出站负载只由强类型参数序列化而来，类型层面不存在
//!   `absolute_path`/`audio_bytes`/`cover_bytes`；
//! - **先自止而非被 kill**：插件内部 HTTP 超时 8s，早于 Host 的 10–30s 窗口；
//! - **本仓零文件系统写入**：插件只返回 Suggestion（铁律）。

pub mod ai_openai;
pub mod cover_itunes;
pub mod lyrics_lrclib;

use std::io::{BufRead, Write};
use std::time::Instant;

use musicforge_plugin_api::{methods, PluginError, PluginManifest, Request, Response};

/// 插件内部 HTTP 超时（秒）——必须早于 Host 10s 下限，先自止而非被 kill。
pub const HTTP_TIMEOUT_SECS: u64 = 8;

/// 能力方法处理器：方法名 + 参数 → 结果 或 (稳定码, 消息)。
pub type Handler = fn(&str, serde_json::Value) -> Result<serde_json::Value, (String, String)>;

/// 组装 plugin.json 契约清单（避免每个 bin 手写漂移）。
pub fn manifest_for(name: &str, data_sent: &[&str]) -> PluginManifest {
    PluginManifest {
        name: name.to_string(),
        api_version: "1.0.0".to_string(),
        kind: musicforge_plugin_api::PluginKind::Ai,
        network: true,
        data_sent: data_sent.iter().map(|s| s.to_string()).collect(),
        data_not_sent: vec![
            "absolute_path".to_string(),
            "audio_bytes".to_string(),
            "cover_bytes".to_string(),
        ],
        // AI/在线插件属 L1/L2 低风险：无需 ACK 闸（P6b：格式迁移类才需要）
        ack_required: false,
    }
}

/// X8 NDJSON 服务环：读一行 → 处理 → 写一行。stdin 关闭即退出。
///
/// `plugin.manifest/health/shutdown` 生命周期方法在此统一实现；
/// 其余方法交 `handler`；未知方法 → `MF-PLUGIN-METHOD-UNKNOWN`。
pub fn serve(name: &str, data_sent: &[&str], handler: Handler) {
    let manifest = manifest_for(name, data_sent);
    let started = Instant::now();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(line.trim()) {
            Err(_) => Response::err("unknown", "MF-PLUGIN-MANIFEST-INVALID", "请求行无法解析"),
            Ok(req) => {
                let resp = match req.method.as_str() {
                    methods::PLUGIN_MANIFEST => {
                        Response::ok(&req.id, serde_json::to_value(&manifest).unwrap())
                    }
                    methods::PLUGIN_HEALTH => Response::ok(
                        &req.id,
                        serde_json::json!({
                            "status": "ok",
                            "uptime_ms": started.elapsed().as_millis() as u64,
                        }),
                    ),
                    methods::PLUGIN_SHUTDOWN => {
                        let resp = Response::ok(
                            &req.id,
                            serde_json::to_value(&musicforge_plugin_api::ShutdownResult {
                                accepted: true,
                            })
                            .unwrap(),
                        );
                        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
                        let _ = out.flush();
                        std::process::exit(0);
                    }
                    method => match handler(method, req.params) {
                        Ok(v) => Response::ok(&req.id, v),
                        Err((code, message)) => Response::err(&req.id, &code, message),
                    },
                };
                resp
            }
        };
        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
        let _ = out.flush();
    }
}

/// 未知方法的稳定错误（各插件 handler 对不服务的方法统一返回）。
pub fn unknown_method(method: &str) -> (String, String) {
    (
        "MF-PLUGIN-METHOD-UNKNOWN".to_string(),
        format!("本插件不服务该方法: {method}"),
    )
}

// ---------------------------------------------------------------- HTTP 层 --

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
}

/// GET JSON（非 2xx / 超时 / 解析失败 → 显式 Err，绝不静默空结果）。
pub fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let text = agent()
        .get(url)
        .call()
        .map_err(|e| format!("HTTP GET 失败: {e}"))?
        .into_string()
        .map_err(|e| format!("HTTP 响应读取失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("响应 JSON 解析失败: {e}"))
}

/// POST JSON（Bearer 认证；非 2xx → Err 并附响应体摘要）。
pub fn http_post_json(
    url: &str,
    bearer: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let resp = agent()
        .post(url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .send_json(body)
        .map_err(|e| {
            // 4xx 时 ureq 会带响应体；摘要附进错误（显式可见，绝不伪装成功）
            let detail = match e {
                ureq::Error::Status(code, resp) => {
                    let body = resp.into_string().unwrap_or_default();
                    format!("HTTP {code}: {}", &body[..body.len().min(300)])
                }
                other => format!("{other}"),
            };
            format!("HTTP POST 失败: {detail}")
        })?;
    resp.into_json::<serde_json::Value>()
        .map_err(|e| format!("响应 JSON 解析失败: {e}"))
}

/// API Key 缺失的稳定错误（显式可见，绝不静默降级为假结果）。
pub fn missing_key(env: &str) -> (String, String) {
    (
        "MF-PLUGIN-PERMISSION-DENIED".to_string(),
        format!("环境变量 {env} 未设置：本方法需要 API Key。请在启动主程序前配置。"),
    )
}

#[allow(dead_code)]
fn _type_witness(_: PluginError) {}
