use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "macos")]
mod macos;

static OPEN_FILES: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

pub fn install_file_open_handler() {
    #[cfg(target_os = "macos")]
    macos::install();
}

pub fn install_native_menu() {
    #[cfg(target_os = "macos")]
    macos::install_native_menu();
}

pub fn take_open_files() -> Vec<PathBuf> {
    OPEN_FILES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut files| std::mem::take(&mut *files))
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn enqueue_open_files(files: impl IntoIterator<Item = PathBuf>) {
    if let Ok(mut queue) = OPEN_FILES.get_or_init(|| Mutex::new(Vec::new())).lock() {
        queue.extend(files);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{OPEN_FILES, take_open_files};

    #[test]
    fn drains_queued_open_files() {
        let mut queue = OPEN_FILES
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap();
        queue.clear();
        queue.push(PathBuf::from("example.md"));
        drop(queue);

        assert_eq!(take_open_files(), vec![PathBuf::from("example.md")]);
        assert!(take_open_files().is_empty());
    }
}
