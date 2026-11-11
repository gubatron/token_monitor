//! Copies plain-Rust panel view models into the generated Slint components.
//!
//! Kept mechanical on purpose: no strings or numbers are decided here.

use slint::{Color, ModelRc, SharedString, VecModel};
use tokenmon_core::{ProviderId, Srgb};

use super::claude_chatgpt::{ChatGptUsage, ClaudePanelVm, ClaudeUsage};
use super::common::{
    DailyBudgetVm, DayBarVm, FooterVm, HeaderVm, HoverCardVm, HoverVm, MetricVm, ModelsCardVm,
    PanelVm, TrackVm, geometry,
};
use super::grok::GrokUsage;
use super::grokbot_cursor::{CursorUsage, GrokbotUsage};
use super::opencode_openrouter::{OpenCodeUsage, OpenRouterPanelVm};
use super::overview::OverviewPanelVm;
use super::switcher::RowVm;
use super::ui;

fn color(c: Srgb) -> Color {
    let (r, g, b) = c.to_rgb8();
    Color::from_argb_u8((c.alpha.clamp(0.0, 1.0) * 255.0).round() as u8, r, g, b)
}

fn text(s: &str) -> SharedString {
    SharedString::from(s)
}

/// Section headers are drawn uppercased (`PanelSectionHeader`).
fn upper(s: &str) -> SharedString {
    SharedString::from(s.to_uppercase())
}

fn opt(s: Option<&str>) -> SharedString {
    SharedString::from(s.unwrap_or(""))
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn header(h: &HeaderVm) -> ui::HeaderData {
    ui::HeaderData {
        logo_key: opt(h.provider.logo_asset_key()),
        title: text(&h.title),
        meta: opt(h.meta.as_deref()),
        accent: color(h.provider.accent()),
    }
}

fn track(t: &TrackVm) -> ui::TrackData {
    ui::TrackData {
        label: text(&t.label),
        show_label: t.show_label,
        percent: (t.fraction * 100.0) as f32,
        color: color(t.color),
        caption: opt(t.caption.as_deref()),
        caption_warning: t.caption_is_warning,
    }
}

fn tracks(v: &[TrackVm]) -> ModelRc<ui::TrackData> {
    model(v.iter().map(track).collect())
}

fn metrics(v: &[MetricVm]) -> ModelRc<ui::MetricItem> {
    model(
        v.iter()
            .map(|m| ui::MetricItem {
                title: text(&m.title),
                value: text(&m.value),
            })
            .collect(),
    )
}

/// Label/value rows as a Slint model.
fn rows(pairs: &[(String, String)]) -> ModelRc<ui::LabelValue> {
    model(
        pairs
            .iter()
            .map(|(l, v)| ui::LabelValue {
                label: text(l),
                value: text(v),
            })
            .collect(),
    )
}

/// The shared hover label content (Slint `HoverCardData`).
fn hover_card(c: &HoverCardVm) -> ui::HoverCardData {
    ui::HoverCardData {
        title: text(&c.title),
        caption: opt(c.caption.as_deref()),
        total: opt(c.total.as_deref()),
        value: opt(c.value.as_deref()),
        rows: rows(&c.rows),
        extra_rows: rows(&c.extra_rows),
        note: opt(c.note.as_deref()),
        verdict: opt(c.verdict.as_deref()),
    }
}

/// A day's hover as the shared card: title and usage line, then the token
/// block (total, value, breakdown) and the top models.
fn day_hover(h: &HoverVm) -> HoverCardVm {
    let tokens = h.tokens.as_ref();
    HoverCardVm {
        title: h.title.clone(),
        caption: h.usage_line.clone(),
        total: tokens.map(|t| t.total_text.clone()),
        value: tokens.and_then(|t| t.cost_text.clone()),
        rows: tokens.map(|t| t.rows.clone()).unwrap_or_default(),
        extra_rows: tokens.map(|t| t.models.clone()).unwrap_or_default(),
        note: None,
        verdict: None,
    }
}

/// Kit `DayBar` for one rendered column: the percent is recovered from the
/// fill height (`min(68, max(6, 68 * pct / 100))` is its own inverse).
fn day_bar(b: &DayBarVm) -> ui::DayBar {
    ui::DayBar {
        fill_percent: (b.fill_height / geometry::STEM_HEIGHT * 100.0) as f32,
        show_fill: b.fill_height > 0.0,
        prior_window: b.fill_opacity < 1.0,
        future: b.label_opacity < 1.0,
        label_percent: text(&b.value_label),
        label_weekday: text(&b.weekday),
        hover: hover_card(&day_hover(&b.hover)),
    }
}

fn models(m: Option<&ModelsCardVm>) -> ui::ModelsData {
    let Some(m) = m else {
        return ui::ModelsData {
            present: false,
            title: SharedString::new(),
            collapsible: false,
            expanded: false,
            toggle: SharedString::new(),
            rows: model(Vec::new()),
        };
    };
    ui::ModelsData {
        present: true,
        title: upper(&m.title),
        collapsible: m.collapsible,
        expanded: m.expanded,
        toggle: opt(m.toggle_label),
        rows: model(
            m.rows
                .iter()
                .map(|r| ui::ModelRowData {
                    name: text(&r.name),
                    cost: text(&r.cost_text),
                    tag: opt(r.tag.as_deref()),
                    share: r.share_fraction as f32,
                    color: color(r.color),
                    hover: hover_card(&r.hover.clone().unwrap_or_default()),
                })
                .collect(),
        ),
    }
}

fn daily(d: Option<&DailyBudgetVm>) -> ui::DailyData {
    let Some(d) = d else {
        return ui::DailyData {
            present: false,
            title: SharedString::new(),
            allowance: SharedString::new(),
            range: SharedString::new(),
            has_nav: false,
            can_next: false,
            accent: Color::default(),
            bars: model(Vec::new()),
            footer: SharedString::new(),
        };
    };
    ui::DailyData {
        present: true,
        title: upper(&d.title),
        allowance: opt(d.allowance_text.as_deref()),
        range: text(&d.range_label),
        has_nav: d.has_navigation,
        can_next: d.can_go_next,
        accent: d.bars.first().map_or(Color::default(), |b| color(b.color)),
        bars: model(d.bars.iter().map(day_bar).collect()),
        footer: opt(d.footer.as_deref()),
    }
}

fn footer(f: &FooterVm, error_text: Option<&str>) -> ui::FooterData {
    ui::FooterData {
        stale_message: opt(f.stale.as_ref().map(|s| s.message.as_str())),
        stale_age: opt(f.stale.as_ref().and_then(|s| s.age_line.as_deref())),
        sign_in_again: opt(f.sign_in_again.as_ref().map(|a| a.label.as_str())),
        error_text: opt(error_text),
        sign_out_confirm: opt(f.sign_out.as_ref().map(|s| s.confirm_text.as_str())),
    }
}

fn frame_text<U>(p: &PanelVm<U>) -> (SharedString, SharedString) {
    (
        opt(p.message.as_deref()),
        opt(p.sign_in.as_ref().map(|a| a.label.as_str())),
    )
}

/// Converts switcher rows to Slint data.
pub fn tab_rows(rows: &[RowVm]) -> ModelRc<ui::TabRowData> {
    model(
        rows.iter()
            .map(|r| ui::TabRowData {
                top_divider: r.top_divider,
                cells: model(
                    r.cells
                        .iter()
                        .map(|c| ui::TabData {
                            label: opt(c.tab.as_ref().map(|t| t.label)),
                            logo_key: opt(c.tab.as_ref().and_then(|t| t.logo)),
                            selected: c.tab.as_ref().is_some_and(|t| t.selected),
                            present: c.tab.is_some(),
                            divider: c.leading_divider,
                        })
                        .collect(),
                ),
            })
            .collect(),
    )
}

/// The body of the one panel window that shows `provider`.
pub fn provider_tab(provider: ProviderId) -> ui::ProviderTab {
    match provider {
        ProviderId::Overview => ui::ProviderTab::Overview,
        ProviderId::Grok => ui::ProviderTab::Grok,
        ProviderId::Grokbot => ui::ProviderTab::Grokbot,
        ProviderId::Cursor => ui::ProviderTab::Cursor,
        ProviderId::Claude => ui::ProviderTab::Claude,
        ProviderId::ChatGpt => ui::ProviderTab::Chatgpt,
        ProviderId::OpenCode => ui::ProviderTab::Opencode,
        ProviderId::OpenRouter => ui::ProviderTab::Openrouter,
    }
}

/// Fills a [`ui::GrokVm`].
pub fn apply_grok(w: &ui::GrokVm<'_>, p: &PanelVm<GrokUsage>) {
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_pool_used(text(&u.pool_used_text));
        w.set_segments(model(
            u.segments
                .iter()
                .map(|s| ui::BarSegment {
                    width: s.width as f32,
                    color: s.color.map_or(Color::default(), color),
                    remainder: s.color.is_none(),
                })
                .collect(),
        ));
        w.set_reset_caption(opt(u.reset_caption.as_deref()));
        w.set_has_categories(u.categories.is_some());
        w.set_chips(model(
            u.categories
                .iter()
                .flatten()
                .map(|c| ui::CategoryItem {
                    name: text(&c.name),
                    percent_text: text(&c.percent_text),
                    color: color(c.color),
                })
                .collect(),
        ));
        w.set_extra_credits(opt(u.extra_credits.as_deref()));
        w.set_daily(daily(u.daily.as_ref()));
        w.set_footer(footer(&u.footer, None));
    }
}

/// Fills a [`ui::GrokbotVm`].
pub fn apply_grokbot(w: &ui::GrokbotVm<'_>, p: &PanelVm<GrokbotUsage>) {
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_pool_used(text(&u.pool_used_text));
        w.set_entitlement(text(&u.entitlement_caption));
        w.set_tracks(model(u.track.iter().map(track).collect()));
        w.set_no_allowance(opt(u.no_allowance_text));
        w.set_daily(daily(u.daily.as_ref()));
        w.set_footer(footer(&u.footer, None));
    }
}

/// Fills a [`ui::CursorVm`].
pub fn apply_cursor(w: &ui::CursorVm<'_>, p: &PanelVm<CursorUsage>) {
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_tracks(tracks(&u.tracks));
        w.set_daily(daily(u.daily.as_ref()));
        w.set_has_metrics(u.metrics.is_some());
        w.set_metrics(metrics(u.metrics.as_deref().unwrap_or(&[])));
        w.set_footer(footer(&u.footer, None));
    }
}

/// Fills a [`ui::ClaudeVm`].
pub fn apply_claude(w: &ui::ClaudeVm<'_>, v: &ClaudePanelVm) {
    let p: &PanelVm<ClaudeUsage> = &v.panel;
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_tracks(tracks(&u.tracks));
        w.set_daily(daily(u.daily.as_ref()));
        w.set_footer(footer(&u.footer, None));
    }
    w.set_has_code(v.code.is_some());
    if let Some(c) = &v.code {
        w.set_code_metrics(metrics(&c.metrics));
        w.set_code_models(models(c.models.as_ref()));
    }
}

/// Fills a [`ui::ChatGptVm`].
pub fn apply_chatgpt(w: &ui::ChatGptVm<'_>, p: &PanelVm<ChatGptUsage>) {
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_tracks(tracks(&u.tracks));
        w.set_blocked(opt(u.blocked_text));
        w.set_footer(footer(&u.footer, None));
    }
}

/// Fills a [`ui::OpenCodeVm`].
pub fn apply_opencode(w: &ui::OpenCodeVm<'_>, p: &PanelVm<OpenCodeUsage>) {
    let (message, button) = frame_text(p);
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(message);
    w.set_button(button);
    if let Some(u) = &p.usage {
        w.set_pool_title(upper(&u.pool_title));
        w.set_tracks(tracks(&u.tracks));
        w.set_estimate_note(opt(u.estimate_note));
        w.set_daily(daily(u.daily.as_ref()));
        w.set_models(models(u.models.as_ref()));
        w.set_has_metrics(u.metrics.is_some());
        w.set_metrics(metrics(u.metrics.as_deref().unwrap_or(&[])));
        w.set_sign_in_label(opt(u.sign_in.as_ref().map(|a| a.label.as_str())));
        w.set_footer(footer(&u.footer, u.error_text.as_deref()));
    }
}

/// Fills a [`ui::OpenRouterVm`].
pub fn apply_openrouter(w: &ui::OpenRouterVm<'_>, v: &OpenRouterPanelVm) {
    let p = &v.panel;
    w.set_header(header(&p.header));
    w.set_has_usage(p.usage.is_some());
    w.set_message(opt(p.message.as_deref()));
    w.set_has_key_entry(v.key_entry.is_some());
    w.set_key_draft(text(&v.key_draft));
    if let Some(e) = &v.key_entry {
        w.set_key_placeholder(text(e.placeholder));
        w.set_key_button(text(e.button_label));
        w.set_key_button_enabled(e.button_enabled);
        w.set_key_error(opt(e.error.as_deref()));
    }
    if let Some(u) = &p.usage {
        w.set_budget_title(upper(u.budget_title));
        w.set_used_text(opt(u.used_text.as_deref()));
        w.set_tracks(model(u.track.iter().map(track).collect()));
        w.set_no_limit(opt(u.no_limit_text));
        w.set_stats(metrics(&u.stats));
        w.set_models(models(u.models.as_ref()));
        w.set_replace_link(text(if u.replace_link {
            "Replace API Key…"
        } else {
            ""
        }));
        let f = FooterVm {
            stale: u.stale.clone(),
            sign_in_again: None,
            sign_out: v.sign_out.clone(),
        };
        w.set_footer(footer(&f, None));
    }
}

/// Fills a [`ui::OverviewVm`].
pub fn apply_overview(w: &ui::OverviewVm<'_>, v: &OverviewPanelVm) {
    w.set_version(text(&v.version_text));
    w.set_cards(model(
        v.cards
            .iter()
            .map(|c| ui::OverviewCardData {
                logo_key: opt(c.provider.logo_asset_key()),
                title: text(&c.title),
                meta: text(&c.meta.to_uppercase()),
                used_text: text(&c.used_text),
                percent: (c.fraction * 100.0) as f32,
                color: color(c.color),
                caption: opt(c.caption.as_deref()),
                caption_tertiary: c.caption_is_tertiary,
                sign_in: opt(c.sign_in.as_ref().map(|s| s.0.as_str())),
                has_sign_out: c.sign_out.is_some(),
            })
            .collect(),
    ));
    w.set_daily_title(upper(&v.daily.title));
    w.set_has_nav(v.daily.has_navigation);
    w.set_can_back(v.daily.can_go_back);
    w.set_can_forward(v.daily.can_go_forward);
    w.set_empty_message(opt(v.daily.chart.empty_message));
    w.set_hours(model(
        v.daily
            .chart
            .columns
            .iter()
            .map(|c| ui::HourBarData {
                bar_height: c.bar_height as f32,
                segments: model(
                    c.segments
                        .iter()
                        .map(|s| ui::HourSegData {
                            height: s.height as f32,
                            color: color(s.color),
                        })
                        .collect(),
                ),
                axis_label: opt(c.axis_label.as_deref()),
                tooltip: text(&c.tooltip),
            })
            .collect(),
    ));
    w.set_legend(model(
        v.daily
            .chart
            .legend
            .iter()
            .map(|l| ui::LegendData {
                title: text(l.title),
                color: color(l.color),
            })
            .collect(),
    ));
}
