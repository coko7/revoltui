use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, HighlightSpacing, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};

use crate::app::{App, Focus, PaneRows};
use crate::data::{self, Bucket, Event, Origin};

const ALL_CITIES: &str = "Toutes les villes";
const TAG_MAX: usize = 25;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let name_w = app
        .cities
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(ALL_CITIES.len(), 26);
    let cities_w = (name_w as u16 + 12).min(body.width / 2);
    let [left, right] =
        Layout::horizontal([Constraint::Length(cities_w), Constraint::Min(0)]).areas(body);
    let [events, detail] =
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(right);
    app.rows = PaneRows {
        cities: left.height.saturating_sub(2),
        events: events.height.saturating_sub(2),
        detail: detail.height.saturating_sub(2),
    };

    draw_header(f, app, header);
    draw_cities(f, app, left, name_w);
    draw_events(f, app, events);
    draw_detail(f, app, detail);
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let generated = app.feed.generated();
    let mut meta = format!(
        " · données du {}/{} {} · collecteur : {}",
        generated.get(8..10).unwrap_or("??"),
        generated.get(5..7).unwrap_or("??"),
        generated.get(11..16).unwrap_or(""),
        app.feed.collector_status(),
    );
    match &app.opts.origin {
        Origin::File(path) => {
            if let Some(age) = data::file_age(path) {
                meta += &format!(" · copie locale : {} min", age.as_secs() / 60);
            }
        }
        Origin::Stdin => meta += " · stdin",
    }
    if app.opts.offline {
        meta += " · hors ligne";
    }
    let mut spans = vec![Span::raw("manifsendirect.fr").bold(), Span::raw(meta).dim()];
    if app.is_refreshing() {
        spans.push(Span::raw(" · mise à jour…").yellow());
    }
    f.render_widget(Line::from(spans).centered(), area);
}

fn draw_cities(f: &mut Frame, app: &mut App, area: Rect, name_w: usize) {
    let visible = app.visible_cities();
    let (now, next) = visible
        .iter()
        .fold((0, 0), |(n, x), c| (n + c.now, x + c.next));
    let mut items = vec![city_row(ALL_CITIES, now, next, name_w, true)];
    items.extend(visible.iter().map(|c| {
        let name = if c.name.is_empty() {
            "(ville n.c.)"
        } else {
            &c.name
        };
        city_row(name, c.now, c.next, name_w, false)
    }));

    let mut title = vec![Span::raw(" Villes ").bold()];
    if app.searching || !app.filter.is_empty() {
        title.push(Span::raw(format!("/{}", app.filter)).yellow());
        if app.searching {
            title.push(Span::raw(" ").reversed());
        }
        title.push(Span::raw(" "));
    }
    let legend = Line::from(vec![
        Span::raw(" en cours").red(),
        Span::raw(" · ").dim(),
        Span::raw("à venir ").blue(),
    ])
    .right_aligned();

    let focused = app.focus == Focus::Cities;
    let list = List::new(items)
        .block(pane(Line::from(title), focused).title_bottom(legend))
        .highlight_style(highlight(focused))
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, area, &mut app.city_state);
}

fn city_row(name: &str, now: usize, next: usize, w: usize, bold: bool) -> ListItem<'static> {
    let count = |n: usize, color: Color| {
        let s = Span::raw(format!("{n:>3}"));
        if n > 0 { s.fg(color) } else { s.dim() }
    };
    let name = Span::raw(format!("{:<w$}", truncate(name, w)));
    ListItem::new(Line::from(vec![
        if bold { name.bold() } else { name },
        Span::raw(" "),
        count(now, Color::Red),
        Span::raw(" "),
        count(next, Color::Blue),
    ]))
}

fn draw_events(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Events;
    let title = match app.selected_city() {
        Some(city) => format!(" {city} "),
        None if app.filter.is_empty() => format!(" {ALL_CITIES} "),
        None => format!(" Villes « {} » ", app.filter),
    };
    let block = pane(Line::from(title).bold(), focused);

    if app.visible_cities().is_empty() {
        let msg = if app.filter.is_empty() {
            "Aucun événement en cours ou à venir.".to_string()
        } else {
            format!("Aucun événement pour « {} ».", app.filter)
        };
        f.render_widget(Paragraph::new(msg.dim()).block(block), area);
        return;
    }

    let today = app.feed.today().to_string();
    let sections = app.sections();
    let all = sections.iter().flat_map(|(_, evs)| evs.iter());
    let tag_w = all
        .clone()
        .map(|e| e.tag().chars().count() + 2)
        .max()
        .unwrap_or(0)
        .min(TAG_MAX);
    let when_w = all
        .map(|e| e.when(&today).chars().count())
        .max()
        .unwrap_or(0);

    let mut items = Vec::new();
    let mut selected = None;
    let mut idx = 0;
    for (bucket, evs) in &sections {
        let heading = format!("━━ {} ({})", bucket.heading(), evs.len());
        items.push(ListItem::new(Line::from(Span::styled(
            heading,
            section_style(*bucket),
        ))));
        if evs.is_empty() {
            items.push(ListItem::new(Line::from("   rien de signalé".dim())));
        }
        for e in evs {
            if idx == app.event_sel {
                selected = Some(items.len());
            }
            items.push(event_row(e, &today, tag_w, when_w));
            idx += 1;
        }
    }

    app.event_state.select(selected);
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight(focused))
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(list, area, &mut app.event_state);
}

fn event_row(e: &Event, today: &str, tag_w: usize, when_w: usize) -> ListItem<'static> {
    let faded = e.probably_ended() || e.bucket() == Bucket::Ended;
    let title = Span::raw(e.title.clone());
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:<tag_w$}", truncate(&format!("[{}]", e.tag()), tag_w)),
            type_style(&e.kind),
        ),
        Span::raw(" "),
        Span::raw(format!("{:<when_w$}", e.when(today))).bold(),
        Span::raw("  "),
        if faded { title.dim() } else { title },
    ]))
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let block = pane(Line::from(" Détail ").bold(), app.focus == Focus::Detail);
    let Some(lines) = app
        .selected_event()
        .map(|e| detail_lines(e, app.feed.today()))
    else {
        f.render_widget(Paragraph::new("—".dim()).block(block), area);
        return;
    };

    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize);
    app.detail_scroll = app
        .detail_scroll
        .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    f.render_widget(paragraph.block(block).scroll((app.detail_scroll, 0)), area);

    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll + 1)
            .position(app.detail_scroll as usize)
            .viewport_content_length(inner.height as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        let track = area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        });
        f.render_stateful_widget(scrollbar, track, &mut state);
    }
}

fn detail_lines(e: &Event, today: &str) -> Vec<Line<'static>> {
    let mut head = vec![
        Span::styled(format!("[{}]", e.tag()), type_style(&e.kind)),
        Span::raw("  "),
        Span::raw(e.when(today)).bold(),
    ];
    if let Some(end) = e.end_hint() {
        head.push(Span::raw(" "));
        head.push(Span::raw(end).bold());
    }
    if e.probably_ended() {
        head.push(Span::raw("  (probablement terminé)").dim());
    }

    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::raw(format!("{label:<6}")).dim(),
            Span::raw(value),
        ])
    };
    let mut lines = vec![
        Line::from(head),
        Line::from(Span::raw(e.title.clone()).bold()),
    ];
    if !e.city.is_empty() {
        lines.push(field("ville", e.city.clone()));
    }
    let location = e.location();
    if !location.is_empty() {
        lines.push(field("lieu", location));
    }
    if let Some(crowd) = e.crowd_label() {
        lines.push(field("foule", crowd));
    }
    if let Some(summary) = e.summary.as_deref().filter(|s| !s.is_empty()) {
        lines.push(Line::default());
        lines.push(Line::raw(summary.to_string()));
    }
    lines.push(Line::default());
    let mut sources = vec![if e.confirmed() {
        Span::raw("✓ confirmé").green()
    } else {
        Span::raw("source unique").dim()
    }];
    let names = e.source_names();
    if !names.is_empty() {
        sources.push(Span::raw(format!(" · {names}")).dim());
    }
    lines.push(Line::from(sources));
    if let Some(url) = e.latest_url() {
        lines.push(Line::from(Span::raw(url.to_string()).dim().underlined()));
    }
    lines
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(status) = &app.status {
        let text = Span::raw(status.text.clone());
        Line::from(if status.error {
            text.yellow()
        } else {
            text.green()
        })
    } else if app.searching {
        keys(&[("Entrée", "valider"), ("Échap", "effacer"), ("↑↓", "ville")])
    } else {
        let ended = if app.show_ended {
            "masquer terminés"
        } else {
            "terminés"
        };
        let arrows = if app.focus == Focus::Detail {
            "défiler"
        } else {
            "naviguer"
        };
        keys(&[
            ("/", "chercher"),
            ("↑↓", arrows),
            ("⇥", "panneau"),
            ("a", ended),
            ("r", "rafraîchir"),
            ("o", "ouvrir"),
            ("q", "quitter"),
        ])
    };
    f.render_widget(line.centered(), area);
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, desc)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(k.to_string()).cyan().bold());
        spans.push(Span::raw(format!(" {desc}")).dim());
    }
    Line::from(spans)
}

fn pane(title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title);
    if focused {
        block.border_style(Style::new().cyan())
    } else {
        block
    }
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().reversed()
    } else {
        Style::new().bold()
    }
}

fn section_style(bucket: Bucket) -> Style {
    match bucket {
        Bucket::Now => Style::new().red().bold(),
        Bucket::Next => Style::new().blue().bold(),
        Bucket::Ended => Style::new().dim().bold(),
    }
}

fn type_style(kind: &str) -> Style {
    match kind {
        "cortege" => Style::new().red(),
        "blocage" => Style::new().magenta(),
        "greve" => Style::new().yellow(),
        "rassemblement" => Style::new().cyan(),
        "incident" => Style::new().red().bold(),
        _ => Style::new().blue(),
    }
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
    t.push('…');
    t
}
