use std::io;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::data::{self, Bucket, CitySummary, Event, Feed, Origin};
use crate::input::Input;
use crate::ui;

/// How often a failed background download may be retried.
const RETRY_EVERY: Duration = Duration::from_secs(60);

pub struct Options {
    pub origin: Origin,
    pub url: String,
    pub max_age: Duration,
    pub offline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Cities,
    Events,
    Detail,
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

/// Inner height of each pane, in rows.
#[derive(Debug, Default, Clone, Copy)]
pub struct PaneRows {
    pub cities: u16,
    pub events: u16,
    pub detail: u16,
}

pub struct App {
    pub feed: Feed,
    pub cities: Vec<CitySummary>,
    pub filter: Input,
    pub searching: bool,
    pub show_ended: bool,
    pub show_cities: bool,
    pub focus: Focus,
    /// Row 0 is "all cities", row `i + 1` is `visible_cities()[i]`.
    pub city_state: ListState,
    pub event_state: ListState,
    /// Index into `flat_events()`.
    pub event_sel: usize,
    /// First visible line of the detail pane; clamped to the content when drawn.
    pub detail_scroll: u16,
    /// Pane sizes at the last draw, for half-page moves.
    pub rows: PaneRows,
    pub status: Option<Status>,
    pub opts: Options,
    pending: Option<Receiver<Result<Feed, String>>>,
    last_attempt: Instant,
    quit: bool,
}

impl App {
    pub fn new(
        feed: Feed,
        opts: Options,
        filter: String,
        show_ended: bool,
        warning: Option<String>,
    ) -> Self {
        let cities = data::city_summaries(&feed);
        Self {
            feed,
            cities,
            filter: Input::new(filter),
            searching: false,
            show_ended,
            show_cities: true,
            focus: Focus::Cities,
            city_state: ListState::default().with_selected(Some(0)),
            event_state: ListState::default(),
            event_sel: 0,
            detail_scroll: 0,
            rows: PaneRows::default(),
            status: warning.map(|text| Status { text, error: true }),
            opts,
            pending: None,
            last_attempt: Instant::now(),
            quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            terminal.draw(|f| ui::draw(f, self))?;
            if event::poll(Duration::from_millis(250))?
                && let event::Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key);
            }
            self.tick();
        }
        Ok(())
    }

    // ---------- derived views ----------

    /// Cities matching the filter that have something to show.
    pub fn visible_cities(&self) -> Vec<&CitySummary> {
        let q = data::fold(self.filter.as_str());
        self.cities
            .iter()
            .filter(|c| c.now + c.next > 0 || (self.show_ended && c.ended > 0))
            .filter(|c| q.is_empty() || data::fold(&c.name).contains(&q))
            .collect()
    }

    /// Selected city name, `None` for the "all cities" row.
    pub fn selected_city(&self) -> Option<&str> {
        let row = self.city_state.selected().unwrap_or(0);
        let i = row.checked_sub(1)?;
        self.visible_cities().get(i).map(|c| c.name.as_str())
    }

    /// Events of the selected city (or of every visible city), grouped by section.
    pub fn sections(&self) -> Vec<(Bucket, Vec<&Event>)> {
        let names: Vec<&str> = match self.selected_city() {
            Some(name) => vec![name],
            None => self
                .visible_cities()
                .iter()
                .map(|c| c.name.as_str())
                .collect(),
        };
        Bucket::ALL
            .into_iter()
            .filter(|&b| b != Bucket::Ended || self.show_ended)
            .map(|b| {
                let mut evs: Vec<&Event> = self
                    .feed
                    .visible()
                    .filter(|e| e.bucket() == b && names.contains(&e.city.as_str()))
                    .collect();
                evs.sort_by(|x, y| data::section_order(b, x, y));
                (b, evs)
            })
            .collect()
    }

    pub fn flat_events(&self) -> Vec<&Event> {
        self.sections()
            .into_iter()
            .flat_map(|(_, evs)| evs)
            .collect()
    }

    pub fn selected_event(&self) -> Option<&Event> {
        self.flat_events().get(self.event_sel).copied()
    }

    pub fn is_refreshing(&self) -> bool {
        self.pending.is_some()
    }

    // ---------- keys ----------

    fn on_key(&mut self, key: KeyEvent) {
        self.status = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.clear_filter();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Up => self.move_city(-1),
                KeyCode::Down => self.move_city(1),
                KeyCode::Char('d') if ctrl => self.move_city(self.half_page()),
                _ => {
                    let before = self.filter.as_str().to_owned();
                    if self.filter.handle(key) && self.filter.as_str() != before {
                        self.filter_changed();
                    }
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('d') if ctrl => self.move_by(self.half_page()),
            KeyCode::Char('u') if ctrl => self.move_by(-self.half_page()),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            KeyCode::Char('/') => {
                self.searching = true;
                self.show_cities = true;
                self.focus = Focus::Cities;
            }
            KeyCode::Char('v') => self.set_show_cities(!self.show_cities),
            KeyCode::Tab => self.shift_focus(1, true),
            KeyCode::BackTab => self.shift_focus(-1, true),
            KeyCode::Left | KeyCode::Char('h') => self.shift_focus(-1, false),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.shift_focus(1, false),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX),
            KeyCode::Char('a') => {
                let keep = self.snapshot();
                self.show_ended = !self.show_ended;
                self.restore(keep);
            }
            KeyCode::Char('r') => self.manual_refresh(),
            KeyCode::Char('o') => self.open_selected(),
            _ => {}
        }
    }

    /// Shows or hides the city list; focus leaves it when hidden.
    pub fn set_show_cities(&mut self, show: bool) {
        self.show_cities = show;
        if !show && self.focus == Focus::Cities {
            self.focus = Focus::Events;
        }
    }

    /// Moves focus `delta` panes to the right, skipping the city list when hidden.
    fn shift_focus(&mut self, delta: isize, wrap: bool) {
        let panes: &[Focus] = if self.show_cities {
            &[Focus::Cities, Focus::Events, Focus::Detail]
        } else {
            &[Focus::Events, Focus::Detail]
        };
        let n = panes.len() as isize;
        let cur = panes.iter().position(|&p| p == self.focus).unwrap_or(0) as isize;
        let next = if wrap {
            (cur + delta).rem_euclid(n)
        } else {
            (cur + delta).clamp(0, n - 1)
        };
        self.focus = panes[next as usize];
    }

    /// Half the focused pane's height, at least one row.
    fn half_page(&self) -> isize {
        let rows = match self.focus {
            Focus::Cities => self.rows.cities,
            Focus::Events => self.rows.events,
            Focus::Detail => self.rows.detail,
        };
        (rows / 2).max(1) as isize
    }

    fn move_by(&mut self, delta: isize) {
        match self.focus {
            Focus::Cities => self.move_city(delta),
            Focus::Events => {
                let n = self.flat_events().len();
                let next = step(self.event_sel, delta, n);
                if next != self.event_sel {
                    self.event_sel = next;
                    self.detail_scroll = 0;
                }
            }
            Focus::Detail => {
                let next = (self.detail_scroll as isize).saturating_add(delta);
                self.detail_scroll = next.clamp(0, u16::MAX as isize) as u16;
            }
        }
    }

    fn move_city(&mut self, delta: isize) {
        let rows = self.visible_cities().len() + 1;
        let cur = self.city_state.selected().unwrap_or(0);
        let next = step(cur, delta, rows);
        if next != cur {
            self.city_state.select(Some(next));
            self.reset_events();
        }
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_changed();
    }

    /// The city list changed: start again from the "all cities" row.
    fn filter_changed(&mut self) {
        self.city_state = ListState::default().with_selected(Some(0));
        self.reset_events();
    }

    fn reset_events(&mut self) {
        self.event_sel = 0;
        self.event_state = ListState::default();
        self.detail_scroll = 0;
    }

    /// Selected city and event id, to find them again after the data changes.
    fn snapshot(&self) -> (Option<String>, Option<String>) {
        (
            self.selected_city().map(str::to_owned),
            self.selected_event().map(|e| e.id.clone()),
        )
    }

    fn restore(&mut self, (city, event): (Option<String>, Option<String>)) {
        let row = city
            .and_then(|c| self.visible_cities().iter().position(|v| v.name == c))
            .map_or(0, |i| i + 1);
        self.city_state.select(Some(row));
        let sel = event.and_then(|id| self.flat_events().iter().position(|e| e.id == id));
        match sel {
            Some(i) => self.event_sel = i,
            None => self.reset_events(),
        }
    }

    fn open_selected(&mut self) {
        let Some(url) = self
            .selected_event()
            .and_then(Event::latest_url)
            .map(str::to_owned)
        else {
            self.set_status("aucun lien pour cet événement", true);
            return;
        };
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let spawned = Command::new(opener)
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                thread::spawn(move || child.wait());
                self.set_status(&format!("ouvert : {url}"), false);
            }
            Err(e) => self.set_status(&format!("{opener} : {e}"), true),
        }
    }

    fn set_status(&mut self, text: &str, error: bool) {
        self.status = Some(Status {
            text: text.to_string(),
            error,
        });
    }

    // ---------- data refresh ----------

    fn set_feed(&mut self, feed: Feed) {
        let keep = self.snapshot();
        self.cities = data::city_summaries(&feed);
        self.feed = feed;
        self.restore(keep);
    }

    fn manual_refresh(&mut self) {
        let Origin::File(path) = &self.opts.origin else {
            self.set_status("lecture depuis stdin : rien à rafraîchir", true);
            return;
        };
        if self.opts.offline {
            match data::read_file(path) {
                Ok(feed) => {
                    self.set_feed(feed);
                    self.set_status("copie locale relue (hors ligne)", false);
                }
                Err(e) => self.set_status(&e, true),
            }
        } else if self.is_refreshing() {
            self.set_status("mise à jour déjà en cours", false);
        } else {
            self.start_download();
        }
    }

    fn start_download(&mut self) {
        let Origin::File(path) = &self.opts.origin else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (url, path) = (self.opts.url.clone(), path.clone());
        thread::spawn(move || {
            let _ = tx.send(data::download(&url, &path));
        });
        self.pending = Some(rx);
        self.last_attempt = Instant::now();
    }

    /// Collects a finished download, or starts one when the local copy got older than `max_age`.
    fn tick(&mut self) {
        if let Some(rx) = &self.pending {
            let outcome = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("download thread stopped".into()),
            };
            self.pending = None;
            match outcome {
                Ok(feed) => {
                    self.set_feed(feed);
                    self.set_status("données mises à jour", false);
                }
                Err(e) => self.set_status(
                    &format!("échec du téléchargement, données conservées — {e}"),
                    true,
                ),
            }
            return;
        }
        if let Origin::File(path) = &self.opts.origin
            && !self.opts.offline
            && self.last_attempt.elapsed() >= RETRY_EVERY
            && data::is_stale(path, self.opts.max_age)
        {
            self.start_download();
        }
    }
}

/// Moves `cur` by `delta` within `0..len`, clamping at both ends.
fn step(cur: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    cur.saturating_add_signed(delta).min(len - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const FIXTURE: &str = r#"{
      "generated_at": "2026-11-10T13:35:06+01:00",
      "config": {"date": "2026-11-10", "collector": {"status_last": "ok"}},
      "events": [
        {"id": "a", "type": "cortege", "subtype": null, "phase": "en_cours",
         "title": "Paris : cortège place de la République", "summary": "Plusieurs milliers de personnes.",
         "city": "Paris", "district": "11e", "place": "Place de la République",
         "starts_at": "2026-11-10T14:00:00+01:00", "expected_end": "2026-11-10T17:00:00+01:00",
         "updated_at": "2026-11-10T13:30:00+01:00", "crowd": {"value": "5 000", "by": "CGT"},
         "confirmation": {"level": "confirme"}, "hidden": false,
         "sources": [{"name": "Le Parisien", "url": "https://a.example/1", "published_at": "2026-11-10T12:00:00+01:00"},
                     {"name": "BFM", "url": "https://b.example/2", "published_at": "2026-11-10T13:00:00+01:00"}]},
        {"id": "b", "type": "greve", "subtype": null, "phase": "a_venir", "title": "Paris : grève RATP",
         "city": "Paris", "starts_at": "2026-11-12T00:00:00+01:00", "updated_at": "2026-11-10T10:00:00+01:00",
         "sources": [{"name": "RATP", "url": "https://c.example/3", "published_at": null}]},
        {"id": "c", "type": "blocage", "subtype": "rond_point", "phase": "a_venir",
         "title": "Saint-Nazaire : blocage du rond-point", "city": "Saint-Nazaire",
         "starts_at": "2026-11-11T06:00:00+01:00", "expected_end": "2026-11-11T14:00:00+01:00",
         "updated_at": "2026-11-10T09:00:00+01:00", "sources": []},
        {"id": "d", "type": "rassemblement", "phase": "termine", "title": "Lyon : rassemblement",
         "city": "Lyon", "starts_at": "2026-11-10T08:00:00+01:00", "updated_at": "2026-11-10T11:00:00+01:00",
         "sources": []},
        {"id": "e", "type": "cortege", "phase": "en_cours", "title": "Caché", "city": "Nulle-Part",
         "hidden": true, "sources": []}
      ]
    }"#;

    fn app(feed: Feed) -> App {
        let opts = Options {
            origin: Origin::Stdin,
            url: String::new(),
            max_age: Duration::ZERO,
            offline: true,
        };
        App::new(feed, opts, String::new(), false, None)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(app: &mut App, s: &str) {
        s.chars().for_each(|c| press(app, KeyCode::Char(c)));
    }

    #[test]
    fn renders_and_navigates() {
        let mut app = app(data::parse(FIXTURE.as_bytes()).unwrap());
        let s = screen(&mut app, 120, 30);
        assert!(
            s.contains("données du 10/11 13:35 · collecteur : ok"),
            "{s}"
        );
        let (first, last) = (s.lines().next().unwrap(), s.lines().last().unwrap());
        assert!(
            first.starts_with("   ") && last.starts_with("   "),
            "header/footer not centered:\n{s}"
        );
        assert!(
            s.contains("EN COURS (1)") && s.contains("À VENIR (2)"),
            "{s}"
        );
        assert!(
            s.contains("Saint-Nazaire") && !s.contains("Lyon") && !s.contains("Nulle-Part"),
            "{s}"
        );
        // first event selected, detail shows it
        assert!(
            s.contains("→ fin estimée ~17:00") && s.contains("5 000 (CGT)"),
            "{s}"
        );
        assert!(
            s.contains("✓ confirmé · BFM, Le Parisien") && s.contains("https://b.example/2"),
            "{s}"
        );

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "st naz");
        press(&mut app, KeyCode::Enter);
        let s = screen(&mut app, 120, 30);
        assert!(
            s.contains("Villes « st naz »") && s.contains("[blocage/rond_point]"),
            "{s}"
        );
        assert!(
            s.contains("mer 11/11 06:00") && s.contains("→ ~14:00"),
            "{s}"
        );
        assert!(!s.contains("grève RATP"), "{s}");

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('a'));
        let s = screen(&mut app, 120, 30);
        assert!(s.contains("Lyon") && s.contains("TERMINÉ (1)"), "{s}");

        // select Paris, then the second event: midnight start shows the day only
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_city(), Some("Paris"));
        assert_eq!(app.selected_event().map(|e| e.id.as_str()), Some("b"));
        let s = screen(&mut app, 120, 30);
        assert!(
            s.contains("jeu 12/11") && s.contains("source unique · RATP"),
            "{s}"
        );

        // selection survives a reload
        app.set_feed(data::parse(FIXTURE.as_bytes()).unwrap());
        assert_eq!(app.selected_event().map(|e| e.id.as_str()), Some("b"));
    }

    #[test]
    fn scrolls_detail() {
        let mut app = app(data::parse(FIXTURE.as_bytes()).unwrap());
        // small terminal: the detail pane only fits a few of its lines
        let s = screen(&mut app, 120, 16);
        assert!(
            s.contains("→ fin estimée ~17:00") && !s.contains("https://b.example/2"),
            "{s}"
        );

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Detail);
        press(&mut app, KeyCode::Char('G'));
        let s = screen(&mut app, 120, 16);
        assert!(
            app.detail_scroll > 0 && app.detail_scroll < u16::MAX,
            "scroll clamped to content"
        );
        assert!(
            !s.contains("→ fin estimée ~17:00") && s.contains("https://b.example/2"),
            "{s}"
        );

        press(&mut app, KeyCode::Char('g'));
        let s = screen(&mut app, 120, 16);
        assert!(s.contains("→ fin estimée ~17:00"), "{s}");

        // picking another event starts its detail from the top
        press(&mut app, KeyCode::Char('G'));
        screen(&mut app, 120, 16);
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn half_page_moves() {
        let ctrl = |app: &mut App, c: char| {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        };
        let mut app = app(data::parse(FIXTURE.as_bytes()).unwrap());
        screen(&mut app, 120, 16);
        assert!(
            app.rows.cities > 4 && app.rows.detail >= 2,
            "{:?}",
            app.rows
        );

        // the city list is shorter than half a page: clamps to the last row
        ctrl(&mut app, 'd');
        assert_eq!(app.city_state.selected(), Some(2));
        ctrl(&mut app, 'u');
        assert_eq!(app.city_state.selected(), Some(0));

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        ctrl(&mut app, 'd');
        assert_eq!(app.detail_scroll, app.rows.detail / 2);
        ctrl(&mut app, 'u');
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn hides_city_list() {
        let mut app = app(data::parse(FIXTURE.as_bytes()).unwrap());
        let legend = "en cours · à venir";
        assert!(screen(&mut app, 120, 30).contains(legend));

        press(&mut app, KeyCode::Char('v'));
        let s = screen(&mut app, 120, 30);
        assert!(!s.contains(legend) && s.contains("EN COURS (1)"), "{s}");
        // the event list takes the full width
        assert!(
            s.lines().nth(1).unwrap().starts_with("┌ Toutes les villes"),
            "{s}"
        );
        assert_eq!(app.focus, Focus::Events, "focus leaves the hidden pane");

        // focus moves skip the hidden pane
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.focus, Focus::Events);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Detail);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Events);

        // searching brings it back
        press(&mut app, KeyCode::Char('/'));
        assert!(app.show_cities && app.focus == Focus::Cities);
        assert!(screen(&mut app, 120, 30).contains(legend));
    }

    #[test]
    fn edits_search_input() {
        let ctrl = |app: &mut App, code: KeyCode| {
            app.on_key(KeyEvent::new(code, KeyModifiers::CONTROL));
        };
        let names = |app: &App| -> Vec<String> {
            app.visible_cities()
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        let mut app = app(data::parse(FIXTURE.as_bytes()).unwrap());
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "paris naz");
        assert!(names(&app).is_empty());

        ctrl(&mut app, KeyCode::Char('w'));
        assert_eq!(app.filter.as_str(), "paris ");
        assert_eq!(names(&app), ["Paris"]);

        // moving the cursor keeps the selected city
        press(&mut app, KeyCode::Down);
        ctrl(&mut app, KeyCode::Left);
        assert_eq!(app.selected_city(), Some("Paris"));

        // typing happens at the cursor, ctrl+backspace (ctrl+h) removes the word before it
        type_str(&mut app, "x");
        assert_eq!(app.filter.as_str(), "xparis ");
        assert!(screen(&mut app, 120, 30).contains(" Villes /xparis "));
        ctrl(&mut app, KeyCode::Char('h'));
        assert_eq!(app.filter.as_str(), "paris ");
        assert_eq!(names(&app), ["Paris"]);
    }
}
