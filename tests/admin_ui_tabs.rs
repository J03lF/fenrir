use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
struct PageState {
    visible: bool,
}

#[derive(Clone, Debug)]
struct ButtonState {
    trigger: &'static str,
    active: bool,
    aria_selected: bool,
}

impl ButtonState {
    fn new(trigger: &'static str) -> Self {
        Self {
            trigger,
            active: false,
            aria_selected: false,
        }
    }
}

#[derive(Default, Debug)]
struct UiState {
    pages: HashMap<&'static str, PageState>,
    buttons: Vec<ButtonState>,
}

impl UiState {
    fn new() -> Self {
        let mut pages = HashMap::new();
        for name in ["overview", "services", "telemetry", "audit"] {
            pages.insert(name, PageState::default());
        }
        let buttons = ["overview", "services", "telemetry", "audit"]
            .into_iter()
            .map(ButtonState::new)
            .collect();
        Self { pages, buttons }
    }

    fn button(&self, trigger: &str) -> &ButtonState {
        self.buttons
            .iter()
            .find(|btn| btn.trigger == trigger)
            .expect("button must exist")
    }
}

#[derive(Default, Debug)]
struct TelemetryCounters {
    raf_requests: u32,
    render_calls: u32,
}

impl TelemetryCounters {
    fn notify_telemetry_tab(&mut self) {
        self.raf_requests += 1;
        self.render_calls += 1;
    }
}

#[derive(Debug)]
struct ChartState {
    placeholder_visible: bool,
    chart_visible: bool,
    message: String,
}

impl ChartState {
    fn new() -> Self {
        Self {
            placeholder_visible: false,
            chart_visible: true,
            message: String::new(),
        }
    }

    fn set_empty(&mut self, empty: bool, message: Option<&str>) {
        if empty {
            self.chart_visible = false;
            self.placeholder_visible = true;
        } else {
            self.chart_visible = true;
            self.placeholder_visible = false;
        }
        if let Some(msg) = message {
            self.message = msg.to_string();
        }
    }
}

fn set_active_page(
    ui: &mut UiState,
    name: &str,
    persist: bool,
    storage: &mut Option<String>,
    writes: &mut Vec<String>,
    telemetry: &mut TelemetryCounters,
) {
    let target = if ui.pages.contains_key(name) {
        name
    } else {
        "overview"
    };

    for (page_name, page) in ui.pages.iter_mut() {
        page.visible = page_name == &target;
    }

    for button in ui.buttons.iter_mut() {
        let active = button.trigger == target;
        button.active = active;
        button.aria_selected = active;
    }

    if persist {
        let value = target.to_string();
        *storage = Some(value.clone());
        writes.push(value);
    }

    if target == "telemetry" {
        telemetry.notify_telemetry_tab();
    }
}

#[test]
fn admin_tab_switching_and_placeholder_smoke() {
    let html = fenrir::infra::http::admin_console_html();
    assert!(html.contains("data-page=\"overview\""));
    assert!(html.contains("data-page=\"telemetry\""));
    assert!(html.contains("data-page=\"audit\""));

    let mut ui = UiState::new();
    let mut storage: Option<String> = None;
    let mut writes_buffer: Vec<String> = Vec::new();
    let mut writes_log: Vec<Vec<String>> = Vec::new();
    let mut telemetry = TelemetryCounters::default();
    let mut chart = ChartState::new();

    set_active_page(
        &mut ui,
        "services",
        true,
        &mut storage,
        &mut writes_buffer,
        &mut telemetry,
    );
    writes_log.push(writes_buffer.clone());
    writes_buffer.clear();

    assert!(ui.pages["services"].visible);
    assert!(!ui.pages["overview"].visible);
    assert!(ui.button("services").active);
    assert_eq!(storage.as_deref(), Some("services"));

    set_active_page(
        &mut ui,
        "telemetry",
        true,
        &mut storage,
        &mut writes_buffer,
        &mut telemetry,
    );
    writes_log.push(writes_buffer.clone());
    writes_buffer.clear();

    assert!(ui.pages["telemetry"].visible);
    assert!(!ui.pages["services"].visible);
    assert!(ui.button("telemetry").active);
    assert_eq!(telemetry.raf_requests, 1);
    assert_eq!(telemetry.render_calls, 1);
    assert_eq!(storage.as_deref(), Some("telemetry"));

    set_active_page(
        &mut ui,
        "overview",
        false,
        &mut storage,
        &mut writes_buffer,
        &mut telemetry,
    );
    writes_log.push(writes_buffer.clone());
    writes_buffer.clear();

    assert!(ui.pages["overview"].visible);
    assert!(!ui.button("telemetry").active);
    assert_eq!(telemetry.raf_requests, 1, "no extra raf on persist=false");

    chart.set_empty(true, Some("Offline"));
    assert!(chart.placeholder_visible);
    assert!(!chart.chart_visible);
    assert_eq!(chart.message, "Offline");

    chart.set_empty(false, None);
    assert!(!chart.placeholder_visible);
    assert!(chart.chart_visible);
    assert_eq!(
        chart.message, "Offline",
        "message persists without override"
    );

    assert_eq!(writes_log.len(), 3);
    assert_eq!(writes_log[0], vec!["services".to_string()]);
    assert_eq!(writes_log[1], vec!["telemetry".to_string()]);
    assert!(writes_log[2].is_empty());
    assert_eq!(storage.as_deref(), Some("telemetry"));
}
