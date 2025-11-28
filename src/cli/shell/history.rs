use rustyline::history::History;
use rustyline::Editor;
use std::fs;
use std::path::PathBuf;

pub(super) fn init_history<H, I>(editor: &mut Editor<H, I>) -> Option<PathBuf>
where
    H: rustyline::Helper,
    I: History,
{
    history_path().inspect(|path| {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = editor.load_history(path);
    })
}

fn history_path() -> Option<PathBuf> {
    let mut path = std::env::var_os("FENRIR_HISTORY_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    path.push("fenrir");
    path.push("cli");
    path.push("history.txt");
    Some(path)
}
