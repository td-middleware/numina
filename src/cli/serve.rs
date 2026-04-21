/// serve 子命令 — 云端 HTTP API Server
///
/// ⚠️  本文件整体被 `#[cfg(feature = "server")]` 包裹。
///    本地构建（cargo build --release）时此文件内容完全不参与编译，
///    numina --help 里也不会出现 serve 子命令。
///
/// 云端构建：cargo build --release --features server
///
/// ─────────────────────────────────────────────────────────────────────────
/// 两大核心能力：
///
///   1. 飞书 Channel 自动处理
///      启动时后台拉起 ChannelDispatcher（lark-cli WebSocket），
///      接收飞书消息 → 意图分析 → chat_react_auto → 自动回复
///
///   2. HTTP API（JSON + SSE 流式）
///      POST /api/v1/chat          普通对话，返回完整 JSON
///      POST /api/v1/chat/stream   流式对话，SSE（text/event-stream）
///      GET  /api/v1/status        服务状态
///      GET  /api/v1/channel/status 飞书 channel 状态
///      GET  /health               健康检查（无需鉴权）
///
/// ─────────────────────────────────────────────────────────────────────────
/// 用法：
///   numina serve                          # 默认 0.0.0.0:8080，自动启飞书 channel
///   numina serve --port 3000              # 自定义端口
///   numina serve --no-lark                # 只开 HTTP API，不启飞书 channel
///   numina serve --lark-buffer 60         # 飞书 Buffer 模式（60s 批量）
///   NUMINA_API_TOKEN=xxx numina serve     # 启用 Bearer token 鉴权

#[cfg(feature = "server")]
pub mod inner {
    use anyhow::Result;
    use clap::Args;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tracing::{info, warn};

    use axum::{
        Router,
        extract::{Json, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Sse, sse::Event},
        routing::{get, post},
    };
    use tower_http::cors::{Any, CorsLayer};
    use tower_http::timeout::TimeoutLayer;
    use tower_http::trace::TraceLayer;

    use crate::channel::{ChannelDispatcher, LarkChannel};
    use crate::channel::types::ProcessingMode;
    use crate::config::{McpFileConfig, ModelsConfig};
    use crate::core::chat::ChatEngine;

    // ─────────────────────────────────────────────
    // CLI 参数
    // ─────────────────────────────────────────────

    #[derive(Args)]
    pub struct ServeArgs {
        /// 监听地址（默认 0.0.0.0）
        #[arg(long, default_value = "0.0.0.0", env = "NUMINA_HOST")]
        pub host: String,

        /// 监听端口
        #[arg(long, short = 'p', default_value = "14521", env = "NUMINA_PORT")]
        pub port: u16,

        /// API 鉴权 Token（Bearer token）
        /// 不设置则不鉴权，建议生产环境通过 NUMINA_API_TOKEN 环境变量设置
        #[arg(long, env = "NUMINA_API_TOKEN")]
        pub api_token: Option<String>,

        /// 普通请求超时秒数（流式接口不受此限制）
        #[arg(long, default_value = "120")]
        pub timeout: u64,

        /// 不自动启动飞书 channel（只开 HTTP API）
        #[arg(long)]
        pub no_lark: bool,

        /// 飞书 Buffer 模式间隔秒数（不指定则 React 模式）
        #[arg(long, value_name = "SECONDS")]
        pub lark_buffer: Option<u64>,

        /// lark-cli 可执行文件路径
        #[arg(long, default_value = "lark-cli", env = "LARK_CLI_PATH")]
        pub lark_cli_path: String,
    }

    // ─────────────────────────────────────────────
    // 共享应用状态
    // ─────────────────────────────────────────────

    #[derive(Clone)]
    pub struct AppState {
        pub engine: Arc<ChatEngine>,
        pub api_token: Option<String>,
        /// 飞书 channel 是否正在运行
        pub lark_running: Arc<RwLock<bool>>,
    }

    // ─────────────────────────────────────────────
    // 请求 / 响应结构体
    // ─────────────────────────────────────────────

    #[derive(serde::Deserialize)]
    pub struct ChatRequest {
        /// 用户消息内容
        pub message: String,
        /// 可选：会话 ID（用于多轮对话，不传则新建会话）
        pub session_id: Option<String>,
        /// 可选：覆盖默认模型
        pub model: Option<String>,
    }

    // ─────────────────────────────────────────────
    // 鉴权辅助
    // ─────────────────────────────────────────────

    fn check_auth(headers: &HeaderMap, api_token: &Option<String>) -> bool {
        let Some(expected) = api_token else {
            return true; // 未配置 token，不鉴权
        };
        let Some(auth_header) = headers.get("Authorization") else {
            return false;
        };
        let Ok(auth_str) = auth_header.to_str() else {
            return false;
        };
        auth_str == format!("Bearer {}", expected)
    }

    // ─────────────────────────────────────────────
    // 路由处理器
    // ─────────────────────────────────────────────

    /// GET /health — 健康检查（无需鉴权，供 k8s/docker 探针使用）
    async fn health() -> impl IntoResponse {
        (StatusCode::OK, Json(serde_json::json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "service": "numina"
        })))
    }

    /// GET /api/v1/status — 服务状态
    async fn api_status(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        if !check_auth(&headers, &state.api_token) {
            return (StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"ok": false, "error": "Unauthorized"})));
        }
        let models = ModelsConfig::load().unwrap_or_default();
        let mcp = McpFileConfig::load().unwrap_or_default();
        let enabled_mcp = mcp.servers.iter().filter(|s| s.enabled).count();
        let lark_running = *state.lark_running.read().await;

        (StatusCode::OK, Json(serde_json::json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "active_model": models.active_model(),
            "models_count": models.models.len(),
            "mcp_servers": { "total": mcp.servers.len(), "enabled": enabled_mcp },
            "channels": { "lark": if lark_running { "running" } else { "stopped" } }
        })))
    }

    /// POST /api/v1/chat — 普通对话（完整 JSON 响应）
    ///
    /// 请求：`{ "message": "你好", "session_id": "可选", "model": "可选" }`
    /// 响应：`{ "ok": true, "reply": "...", "session_id": "...", "model": "..." }`
    async fn api_chat(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(req): Json<ChatRequest>,
    ) -> impl IntoResponse {
        if !check_auth(&headers, &state.api_token) {
            return (StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"ok": false, "error": "Unauthorized"})));
        }
        match state.engine
            .chat_once(&req.message, req.model.as_deref(), req.session_id.as_deref())
            .await
        {
            Ok((reply, sid, _tokens, _ctx)) => {
                let model = state.engine.default_model();
                (StatusCode::OK, Json(serde_json::json!({
                    "ok": true,
                    "reply": reply,
                    "session_id": sid,
                    "model": model
                })))
            }
            Err(e) => {
                warn!("api_chat error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"ok": false, "error": e.to_string()})))
            }
        }
    }

    /// POST /api/v1/chat/stream — 流式对话（SSE）
    ///
    /// 请求体同 /api/v1/chat
    ///
    /// SSE 事件：
    ///   event: delta  data: {"text":"..."}                        ← 流式文本片段
    ///   event: done   data: {"session_id":"...","model":"..."}    ← 完成
    ///   event: error  data: {"error":"..."}                       ← 错误
    async fn api_chat_stream(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(req): Json<ChatRequest>,
    ) -> Sse<std::pin::Pin<Box<dyn futures::Stream<Item = Result<Event, std::convert::Infallible>> + Send>>> {
        // 鉴权失败：返回单个 error 事件
        if !check_auth(&headers, &state.api_token) {
            let stream = async_stream::stream! {
                yield Ok::<Event, std::convert::Infallible>(
                    Event::default().event("error").data(r#"{"error":"Unauthorized"}"#)
                );
            };
            return Sse::new(Box::pin(stream));
        }

        let engine = state.engine.clone();
        let message = req.message.clone();
        let model_override = req.model.clone();
        let session_id = req.session_id.clone();

        let stream = async_stream::stream! {
            match engine.chat_stream(
                &message,
                model_override.as_deref(),
                session_id.as_deref(),
            ).await {
                Err(e) => {
                    yield Ok::<Event, std::convert::Infallible>(
                        Event::default().event("error")
                            .data(serde_json::json!({"error": e.to_string()}).to_string())
                    );
                }
                Ok((mut rx, sid, _sent, _ctx)) => {
                    let mut full_text = String::new();

                    while let Some(chunk) = rx.recv().await {
                        full_text.push_str(&chunk);
                        yield Ok::<Event, std::convert::Infallible>(
                            Event::default().event("delta")
                                .data(serde_json::json!({"text": chunk}).to_string())
                        );
                    }

                    // 流式完成后追加 assistant turn 到 session
                    let _ = ChatEngine::append_assistant_turn(&sid, &full_text);

                    let model = engine.default_model();
                    yield Ok::<Event, std::convert::Infallible>(
                        Event::default().event("done").data(serde_json::json!({
                            "session_id": sid,
                            "model": model,
                            "total_chars": full_text.len()
                        }).to_string())
                    );
                }
            }
        };

        Sse::new(Box::pin(stream))
    }

    /// GET /api/v1/channel/status — 飞书 channel 状态
    async fn api_channel_status(
        State(state): State<AppState>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        if !check_auth(&headers, &state.api_token) {
            return (StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"ok": false, "error": "Unauthorized"})));
        }
        let running = *state.lark_running.read().await;
        (StatusCode::OK, Json(serde_json::json!({
            "ok": true,
            "lark": if running { "running" } else { "stopped" }
        })))
    }

    // ─────────────────────────────────────────────
    // 飞书 Channel 后台任务
    // ─────────────────────────────────────────────

    fn spawn_lark_channel(
        engine: Arc<ChatEngine>,
        lark_running: Arc<RwLock<bool>>,
        cli_path: String,
        buffer_secs: Option<u64>,
    ) {
        tokio::spawn(async move {
            let mode = match buffer_secs {
                Some(secs) => {
                    info!("Lark channel: Buffer mode, interval={}s", secs);
                    ProcessingMode::Buffer { interval_secs: secs }
                }
                None => {
                    info!("Lark channel: React mode");
                    ProcessingMode::React
                }
            };

            let lark_channel = LarkChannel::new().with_cli_path(&cli_path);
            let mut dispatcher = ChannelDispatcher::new();
            dispatcher.register(Box::new(lark_channel), mode);

            { *lark_running.write().await = true; }
            info!("Lark channel dispatcher started");

            if let Err(e) = dispatcher.run(engine).await {
                warn!("Lark channel exited with error: {}", e);
            }

            { *lark_running.write().await = false; }
            info!("Lark channel dispatcher stopped");
        });
    }

    // ─────────────────────────────────────────────
    // 路由构建
    // ─────────────────────────────────────────────

    fn build_router(state: AppState, timeout_secs: u64) -> Router {
        use std::time::Duration;

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        // 普通 API（带超时）
        let api = Router::new()
            .route("/status", get(api_status))
            .route("/chat", post(api_chat))
            .route("/channel/status", get(api_channel_status))
            .layer(TimeoutLayer::new(Duration::from_secs(timeout_secs)));

        // 流式 API（不设超时，由客户端控制）
        let stream_api = Router::new()
            .route("/chat/stream", post(api_chat_stream));

        Router::new()
            .route("/health", get(health))
            .nest("/api/v1", api)
            .nest("/api/v1", stream_api)
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(state)
    }

    // ─────────────────────────────────────────────
    // 优雅退出信号
    // ─────────────────────────────────────────────

    async fn shutdown_signal() {
        use tokio::signal;
        let ctrl_c = async {
            signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
        };
        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler")
                .recv()
                .await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => { info!("Received Ctrl+C, shutting down...") },
            _ = terminate => { info!("Received SIGTERM, shutting down...") },
        }
    }

    // ─────────────────────────────────────────────
    // 执行入口
    // ─────────────────────────────────────────────

    pub async fn execute(args: &ServeArgs) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", args.host, args.port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid address '{}:{}': {}", args.host, args.port, e))?;

        let engine = Arc::new(ChatEngine::new()?);
        info!("ChatEngine initialized, model: {}", engine.default_model());

        let lark_running = Arc::new(RwLock::new(false));

        let state = AppState {
            engine: engine.clone(),
            api_token: args.api_token.clone(),
            lark_running: lark_running.clone(),
        };

        // 后台启动飞书 channel（除非 --no-lark）
        if !args.no_lark {
            spawn_lark_channel(
                engine.clone(),
                lark_running.clone(),
                args.lark_cli_path.clone(),
                args.lark_buffer,
            );
        }

        let router = build_router(state, args.timeout);

        println!("╔══════════════════════════════════════════════════╗");
        println!("║        Numina Server v{}                    ║", env!("CARGO_PKG_VERSION"));
        println!("╠══════════════════════════════════════════════════╣");
        println!("║  监听  : http://{}:{:<26}║", args.host, args.port);
        println!("║  健康  : GET  /health                            ║");
        println!("║  状态  : GET  /api/v1/status                     ║");
        println!("║  对话  : POST /api/v1/chat                       ║");
        println!("║  流式  : POST /api/v1/chat/stream  (SSE)         ║");
        println!("║  Channel: GET /api/v1/channel/status             ║");
        println!("╠══════════════════════════════════════════════════╣");
        if args.api_token.is_some() {
            println!("║  鉴权  : Bearer Token ✓                          ║");
        } else {
            println!("║  鉴权  : 无（建议设置 NUMINA_API_TOKEN）         ║");
        }
        if args.no_lark {
            println!("║  飞书  : 已禁用 (--no-lark)                      ║");
        } else {
            let mode = args.lark_buffer
                .map(|s| format!("Buffer {}s", s))
                .unwrap_or_else(|| "React".to_string());
            println!("║  飞书  : 启动中 ({})                         ║", mode);
        }
        println!("╚══════════════════════════════════════════════════╝");
        println!("  按 Ctrl+C 停止\n");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        println!("\n✅ Numina Server 已停止");
        Ok(())
    }
}

// ─────────────────────────────────────────────
// 公开导出（仅 server feature 下有内容）
// ─────────────────────────────────────────────

#[cfg(feature = "server")]
pub use inner::{ServeArgs, execute};
