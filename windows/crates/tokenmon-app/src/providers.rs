//! Builds the production provider registry (spec 01 section 6.1 steps 5 to 8).
//!
//! Must run inside a tokio runtime: every poller captures the current handle
//! for its polling loop.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::settings::AppSettings;
use tokenmon_core::storage::{AppPaths, DailyQuotaDeltaStore, HourlyDeltaActivityStore, TextStore};
use tokenmon_core::{BoxFuture, Clock, ProviderError, ProviderErrorContext, ProviderId};
use tokenmon_provider_claude::fetch::{ClaudeUsageClient, DEFAULT_BASE_URL};
use tokenmon_provider_claude::poller::{ClaudePollerDeps, ClaudeUsagePoller, live_code_usage};
use tokenmon_provider_cursor::client::CursorUsageClient;
use tokenmon_provider_cursor::http::CursorHttp;
use tokenmon_provider_cursor::poller::{CursorPollerConfig, CursorUsagePoller};
use tokenmon_provider_grok::client::{GrokUsageClient, ReqwestTransport};
use tokenmon_provider_grok::history::{FileBacking, GrokHistoryStore};
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;
use tokenmon_provider_grok::poller::{GrokFetch, GrokUsagePoller, HourlySink};
use tokenmon_provider_grok::provider_poller::GrokProviderPoller;
use tokenmon_provider_opencode::console::{
    ConsoleClient, ConsoleRequest, ConsoleResponse, ConsoleTransport,
};
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_provider_opencode::poller::{
    OpenCodePollerDeps, OpenCodeUsagePoller, console_fetch_fn, live_scan_source,
};

use tokenmon_provider_chatgpt::client::HttpTransport as ChatGptHttp;
use tokenmon_provider_chatgpt::{AccessTokenCache, ChatGptPoller, ChatGptPollerConfig};
use tokenmon_provider_grokbot::{
    GrokbotPollerConfig, GrokbotPublisher, GrokbotUsageClient, GrokbotUsagePoller,
};
use tokenmon_provider_opencode::pricing::{Tokens, catalog_rates};
use tokenmon_provider_openrouter::{
    ClientFactory, CostEstimator, OpenRouterUsageClient, OpenRouterUsagePoller, ReqwestFetcher,
    client::DEFAULT_BASE_URL as OPENROUTER_BASE_URL,
};

use crate::poller::SharedPoller;
use crate::registry::{ProviderEntry, ProviderRegistry};
use crate::sessions::Sessions;
use crate::sources;

/// Why the registry could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The data directory could not be prepared.
    #[error("data directory: {0}")]
    Storage(#[from] std::io::Error),
    /// A provider's HTTP stack could not start.
    #[error("{0}")]
    Provider(#[from] ProviderError),
    /// The TLS backend could not start.
    #[error("http client: {0}")]
    Http(String),
}

/// Everything the registry is built from.
pub struct ProviderDeps<'a> {
    /// App settings (enablement, cadence).
    pub settings: &'a Arc<AppSettings>,
    /// The credential sessions.
    pub sessions: &'a Sessions,
    /// The data directory.
    pub paths: &'a AppPaths,
    /// The time source.
    pub clock: &'a Arc<dyn Clock>,
    /// App version for the `User-Agent`.
    pub version: Option<&'a str>,
    /// The user's home directory (Claude Code transcripts).
    pub home: Option<PathBuf>,
    /// Where OpenCode's local database is looked up.
    pub opencode_env: DataDirEnv,
}

/// Prices zero-cost OpenRouter models from the OpenCode Zen catalogue.
struct ZenEstimator;

impl CostEstimator for ZenEstimator {
    fn estimate(&self, bare_model_id: &str, prompt: i64, completion: i64) -> Option<f64> {
        catalog_rates(bare_model_id).map(|r| {
            r.value(Tokens {
                input: prompt,
                output: completion,
                cache_read: 0,
                cache_write: 0,
            })
        })
    }
}

/// Grok's fetch over the reqwest transport: one client per call, carrying the
/// cookie header of the moment.
struct LiveGrokFetch {
    transport: Arc<ReqwestTransport>,
    clock: Arc<dyn Clock>,
    user_agent: String,
}

impl GrokFetch for LiveGrokFetch {
    fn fetch(
        &self,
        cookie_header: String,
        account_email: Option<String>,
    ) -> BoxFuture<'_, Result<WeeklyUsageSnapshot, ProviderError>> {
        Box::pin(async move {
            GrokUsageClient::new(
                self.transport.clone(),
                Arc::clone(&self.clock),
                self.user_agent.clone(),
                Some(cookie_header),
                account_email,
            )
            .fetch_usage()
            .await
        })
    }
}

/// Feeds Grok's good polls into the hourly activity store.
/// The Grok hourly store the poller records into, readable by the Overview.
pub struct GrokHourly {
    store: Mutex<HourlyDeltaActivityStore>,
    clock: Arc<dyn Clock>,
}

impl GrokHourly {
    /// The store's day start and its 24 hourly weights.
    pub fn read(&self) -> (DateTime<Utc>, [f64; 24]) {
        let store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        (store.day_start(), store.hour_weights())
    }
}

impl HourlySink for GrokHourly {
    fn record(&self, used_percent: f64, at: DateTime<Utc>) {
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(used_percent, at);
    }
    fn clear(&self) {
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear(self.clock.now());
    }
}

/// The OpenCode console GET over reqwest.
struct ReqwestConsole {
    client: reqwest::Client,
    user_agent: String,
}

impl ConsoleTransport for ReqwestConsole {
    fn get<'a>(
        &'a self,
        request: &'a ConsoleRequest,
    ) -> BoxFuture<'a, Result<ConsoleResponse, String>> {
        Box::pin(async move {
            let mut builder = self
                .client
                .get(format!("https://opencode.ai{}", request.path))
                .header("Cookie", &request.cookie)
                .header("Accept", "application/json")
                .header("User-Agent", &self.user_agent)
                .timeout(Duration::from_secs(20));
            if let Some(org) = &request.org_id {
                builder = builder.header("x-org-id", org);
            }
            let response = builder.send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let final_path = response.url().path().to_owned();
            let cf_mitigated = response.headers().contains_key("cf-mitigated");
            let html_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.to_ascii_lowercase().contains("html"));
            let body = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
            let is_html = html_type || body.first() == Some(&b'<');
            Ok(ConsoleResponse {
                status,
                final_path,
                cf_mitigated,
                is_html,
                body,
            })
        })
    }
}

fn entry(provider: ProviderId, poller: SharedPoller, sessions: &Sessions) -> ProviderEntry {
    ProviderEntry {
        provider,
        poller,
        session: sessions.key(provider),
    }
}

/// Builds every poller and the registry, in provider order. Grokbot shares
/// Cursor's session.
///
/// # Errors
/// The data directory or an HTTP stack could not be prepared.
pub fn build_registry(deps: &ProviderDeps<'_>) -> Result<ProviderRegistry, BuildError> {
    let ProviderDeps {
        settings,
        sessions,
        paths,
        clock,
        version,
        ..
    } = deps;
    let text: Arc<dyn TextStore> = Arc::new(paths.string_store("")?);
    let dir = paths.directory()?;
    let user_agent = tokenmon_core::util::user_agent(*version);

    let grok_hourly = Arc::new(GrokHourly {
        store: Mutex::new(HourlyDeltaActivityStore::new(
            Arc::clone(&text),
            "grok_hourly_today",
            Arc::clone(clock),
        )),
        clock: Arc::clone(clock),
    });
    let grok_inner = Arc::new(GrokUsagePoller::new(
        sessions.grok.clone(),
        GrokHistoryStore::open(Some(Box::new(FileBacking::in_dir(&dir))), Arc::clone(clock)),
        Arc::clone(clock),
        Arc::new(LiveGrokFetch {
            transport: Arc::new(
                ReqwestTransport::new().map_err(|e| BuildError::Http(e.to_string()))?,
            ),
            clock: Arc::clone(clock),
            user_agent: user_agent.clone(),
        }),
        Some(Arc::clone(&grok_hourly) as Arc<dyn HourlySink>),
    ));
    let grok_settings = Arc::clone(settings);
    let grok = Arc::new(GrokProviderPoller::new(
        grok_inner,
        Arc::new(move || grok_settings.snapshot()),
        |b| b,
    ));

    let cursor_http =
        CursorHttp::production(ProviderErrorContext::Cursor, *version, Arc::clone(clock))?;
    let (enabled, interval) = (Arc::clone(settings), Arc::clone(settings));
    let cursor = CursorUsagePoller::new(CursorPollerConfig::live(
        sessions.cursor.clone(),
        DailyQuotaDeltaStore::new(Arc::clone(&text), "cursor_daily_usage", Arc::clone(clock)),
        Arc::clone(clock),
        Arc::new(CursorUsageClient::new(cursor_http, Arc::clone(clock))),
        Arc::new(move || enabled.snapshot().is_provider_enabled(ProviderId::Cursor)),
        Arc::new(move |menu_open| {
            interval
                .snapshot()
                .poll_interval(ProviderId::Cursor, menu_open)
        }),
    ));

    let claude_http = tokenmon_core::net::ProviderHttp::for_hosts(
        ProviderErrorContext::Claude,
        *version,
        Arc::clone(clock),
        &sessions.claude.config().hosts,
    )?;
    let home = deps.home.clone().unwrap_or_else(|| dir.clone());
    let claude = ClaudeUsagePoller::new(ClaudePollerDeps {
        settings: Arc::clone(settings),
        auth: sessions.claude.clone(),
        hourly: HourlyDeltaActivityStore::new(
            Arc::clone(&text),
            "claude_hourly_today",
            Arc::clone(clock),
        ),
        daily: DailyQuotaDeltaStore::new(
            Arc::clone(&text),
            "claude_daily_usage",
            Arc::clone(clock),
        ),
        clock: Arc::clone(clock),
        fetch_usage: ClaudeUsageClient::new(claude_http, DEFAULT_BASE_URL)
            .into_fetch_fn(Arc::clone(clock)),
        load_code_usage: live_code_usage(
            tokenmon_provider_claude::discovery::ClaudeEnvironment::new(home),
            &dir,
            Arc::clone(clock),
        ),
    });

    let console = ReqwestConsole {
        client: reqwest::Client::builder()
            .build()
            .map_err(|e| BuildError::Http(e.to_string()))?,
        user_agent,
    };
    let opencode = OpenCodeUsagePoller::new(OpenCodePollerDeps {
        settings: Arc::clone(settings),
        auth: sessions.opencode.clone(),
        clock: Arc::clone(clock),
        fetch_console: console_fetch_fn(Arc::new(ConsoleClient::new(console))),
        load_scan: live_scan_source(deps.opencode_env.clone()),
    });

    let interval_for = |p: ProviderId| {
        let settings = Arc::clone(settings);
        Arc::new(move |menu_open: bool| settings.snapshot().poll_interval(p, menu_open))
            as Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>
    };
    let enabled_for = |p: ProviderId| {
        let settings = Arc::clone(settings);
        Arc::new(move || settings.snapshot().is_provider_enabled(p))
            as Arc<dyn Fn() -> bool + Send + Sync>
    };

    let chatgpt_cache = Arc::new(AccessTokenCache::new());
    let chatgpt_http = tokenmon_core::net::ProviderHttp::for_hosts(
        ProviderErrorContext::ChatGpt,
        *version,
        Arc::clone(clock),
        &sessions.chatgpt.config().hosts,
    )?;
    let chatgpt = ChatGptPoller::new(ChatGptPollerConfig {
        session: sessions.chatgpt.clone(),
        clock: Arc::clone(clock),
        token_cache: Arc::clone(&chatgpt_cache),
        fetch: ChatGptPoller::live_fetch(
            Arc::new(ChatGptHttp::new(chatgpt_http)),
            chatgpt_cache,
            Arc::clone(clock),
        ),
        is_enabled: enabled_for(ProviderId::ChatGpt),
        poll_interval: interval_for(ProviderId::ChatGpt),
    });

    let grokbot = GrokbotUsagePoller::new(GrokbotPollerConfig::live(
        sessions.cursor.clone(),
        GrokbotPublisher::new(&text, Arc::clone(clock)),
        Arc::clone(clock),
        GrokbotUsageClient::production(*version, Arc::clone(clock))?,
        enabled_for(ProviderId::Grokbot),
        interval_for(ProviderId::Grokbot),
    ));

    let (factory_clock, factory_version) = (Arc::clone(clock), version.map(str::to_owned));
    let factory: ClientFactory = Arc::new(move |key| {
        let fetcher = ReqwestFetcher::new(
            OPENROUTER_BASE_URL,
            key.clone(),
            factory_version.as_deref(),
            Arc::clone(&factory_clock),
        )?;
        Ok(OpenRouterUsageClient::new(
            Arc::new(fetcher),
            Arc::new(ZenEstimator),
        ))
    });
    let openrouter = OpenRouterUsagePoller::new(
        Arc::clone(&sessions.openrouter),
        Arc::clone(clock),
        factory,
        enabled_for(ProviderId::OpenRouter),
        interval_for(ProviderId::OpenRouter),
    );

    let registry = ProviderRegistry::new(vec![
        entry(ProviderId::Grok, grok.clone(), sessions),
        entry(ProviderId::Cursor, cursor.clone(), sessions),
        entry(ProviderId::Claude, claude.clone(), sessions),
        entry(ProviderId::OpenCode, opencode.clone(), sessions),
        entry(ProviderId::ChatGpt, chatgpt.clone(), sessions),
        entry(ProviderId::OpenRouter, openrouter.clone(), sessions),
        entry(ProviderId::Grokbot, grokbot.clone(), sessions),
    ]);
    Ok(registry
        .with_view(ProviderId::Grok, sources::grok_source(&grok))
        .with_view(ProviderId::Cursor, sources::cursor_source(&cursor))
        .with_view(ProviderId::Claude, sources::claude_source(&claude))
        .with_view(ProviderId::OpenCode, sources::opencode_source(&opencode))
        .with_view(ProviderId::ChatGpt, sources::chatgpt_source(&chatgpt))
        .with_view(
            ProviderId::OpenRouter,
            sources::openrouter_source(&openrouter),
        )
        .with_view(ProviderId::Grokbot, sources::grokbot_source(&grokbot))
        .with_history(sources::grok_history(&grok))
        .with_hourly(ProviderId::Grok, sources::grok_hourly(&grok_hourly))
        .with_hourly(ProviderId::Cursor, sources::cursor_hourly(&cursor))
        .with_hourly(ProviderId::Claude, sources::claude_hourly(&claude))
        .with_hourly(
            ProviderId::OpenCode,
            sources::opencode_hourly(&opencode, Arc::clone(clock)),
        )
        .with_hourly(ProviderId::Grokbot, sources::grokbot_hourly(&grokbot)))
}
