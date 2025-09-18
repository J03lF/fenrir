use crate::config::AppConfig;

const CLEAR_SCREEN: &str = "\x1B[2J\x1B[H";
const COLOR_RESET: &str = "\x1b[0m";
const COLOR_PRIMARY: &str = "\x1b[38;5;39m";
const COLOR_ACCENT: &str = "\x1b[38;5;214m";
const COLOR_DIM: &str = "\x1b[38;5;244m";
const COLOR_PROMPT: &str = "\x1b[38;5;47m";

const RAW_BANNER: &str = r#"
        ~+

                 *       +
           '                  |
       ()    .-.,="``"=.    - o -
             '=/_       \     |
          *   |  '=._    |
               \     `=./`,        '
            .   '=.__.=' `='      *
   +                         +
        O      *        '       .
"#;

pub struct PromptSet {
    pub main_cli: String,
    pub main_transport: String,
    pub db_cli: String,
    pub db_transport: String,
}

pub fn clear_screen_sequence() -> &'static str {
    CLEAR_SCREEN
}

pub fn banner() -> String {
    format!("{COLOR_PRIMARY}{RAW_BANNER}{COLOR_RESET}")
}

pub fn welcome_line(config: &AppConfig) -> String {
    format!(
        "{dim}Willkommen auf {accent}{server}{dim} - {primary}{app} v{version}{reset}",
        dim = COLOR_DIM,
        accent = COLOR_ACCENT,
        server = config.server.ssh.server_name,
        primary = COLOR_PRIMARY,
        app = config.app.name,
        version = config.app.version,
        reset = COLOR_RESET,
    )
}

pub fn help_hint() -> String {
    format!(
        "{dim}Tippe 'help' zur Anzeige aller verfügbaren Befehle.{reset}",
        dim = COLOR_DIM,
        reset = COLOR_RESET
    )
}

pub fn prompt_set(config: &AppConfig) -> PromptSet {
    let main_transport = format!(
        "[{color}{user}{reset}@{color2}{server}{reset}] {dim}»{reset} ",
        color = COLOR_PROMPT,
        color2 = COLOR_PRIMARY,
        server = config.server.ssh.server_name,
        user = config.server.ssh.user,
        dim = COLOR_DIM,
        reset = COLOR_RESET,
    );
    let db_transport = format!(
        "[{color}{user}{reset}@{color2}{server}{reset}] {color3}db{reset} {dim}»{reset} ",
        color = COLOR_PROMPT,
        color2 = COLOR_PRIMARY,
        color3 = COLOR_ACCENT,
        server = config.server.ssh.server_name,
        user = config.server.ssh.user,
        dim = COLOR_DIM,
        reset = COLOR_RESET,
    );
    let main_cli = wrap_non_print_sequences(&main_transport);
    let db_cli = wrap_non_print_sequences(&db_transport);
    PromptSet {
        main_cli,
        main_transport,
        db_cli,
        db_transport,
    }
}

fn wrap_non_print_sequences(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 8);
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            result.push('\u{1}');
            result.push(ch);
            while let Some(next) = chars.next() {
                result.push(next);
                if next == 'm' {
                    break;
                }
            }
            result.push('\u{2}');
        } else {
            result.push(ch);
        }
    }
    result
}
