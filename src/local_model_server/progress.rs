pub(crate) fn report_progress(
    progress: &Option<LocalProgressReporter>,
    message: impl Into<String>,
) {
    if let Some(callback) = progress {
        callback(message.into());
    }
}

pub(crate) struct TuiDownloadProgress {
    filename: String,
    progress: Option<LocalProgressReporter>,
    state: std::sync::Mutex<DownloadProgressState>,
}

struct DownloadProgressState {
    total: u64,
    current: u64,
    last_percent: Option<u64>,
}

impl TuiDownloadProgress {
    fn new(filename: String, progress: Option<LocalProgressReporter>) -> Self {
        Self {
            filename,
            progress,
            state: std::sync::Mutex::new(DownloadProgressState {
                total: 0,
                current: 0,
                last_percent: None,
            }),
        }
    }

    fn emit(&self, state: &mut DownloadProgressState, force: bool) {
        let percent = state
            .current
            .saturating_mul(100)
            .checked_div(state.total)
            .unwrap_or(0);
        if !force && state.last_percent == Some(percent) {
            return;
        }
        state.last_percent = Some(percent);
        report_progress(
            &self.progress,
            format!(
                "Model · {} · {}% ({}/{})",
                self.filename,
                percent,
                format_bytes(state.current),
                format_bytes(state.total)
            ),
        );
    }
}

impl ProgressHandler for TuiDownloadProgress {
    fn on_progress(&self, event: &ProgressEvent) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(err) => {
                log::warn!("download progress state lock poisoned: {err}");
                return;
            }
        };
        match event {
            ProgressEvent::Download(DownloadEvent::Start { total_bytes, .. }) => {
                state.total = *total_bytes;
                state.current = 0;
                self.emit(&mut state, true);
            }
            ProgressEvent::Download(DownloadEvent::Progress { files }) => {
                if let Some(file) = files.iter().find(|file| file.filename == self.filename) {
                    state.total = file.total_bytes;
                    state.current = file.bytes_completed;
                    self.emit(&mut state, false);
                }
            }
            ProgressEvent::Download(DownloadEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            }) => {
                state.total = *total_bytes;
                state.current = *bytes_completed;
                self.emit(&mut state, false);
            }
            ProgressEvent::Download(DownloadEvent::Complete) => {
                state.current = state.total;
                self.emit(&mut state, true);
            }
            ProgressEvent::Upload(_) => {}
        }
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1}GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1}MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1}KiB", value / KIB)
    } else {
        format!("{bytes}B")
    }
}
