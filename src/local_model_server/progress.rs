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
    total: usize,
    current: usize,
    last_percent: Option<usize>,
}

impl TuiDownloadProgress {
    fn new(filename: String, progress: Option<LocalProgressReporter>) -> Self {
        Self {
            filename,
            progress,
            total: 0,
            current: 0,
            last_percent: None,
        }
    }

    fn emit(&mut self, force: bool) {
        let percent = self
            .current
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(0);
        if !force && self.last_percent == Some(percent) {
            return;
        }
        self.last_percent = Some(percent);
        report_progress(
            &self.progress,
            format!(
                "Model · {} · {}% ({}/{})",
                self.filename,
                percent,
                format_bytes(self.current),
                format_bytes(self.total)
            ),
        );
    }
}

impl HfProgress for TuiDownloadProgress {
    fn init(&mut self, size: usize, filename: &str) {
        self.total = size;
        self.current = 0;
        self.filename = filename.to_string();
        self.emit(true);
    }

    fn update(&mut self, size: usize) {
        self.current = self.current.saturating_add(size);
        self.emit(false);
    }

    fn finish(&mut self) {
        self.current = self.total;
        self.emit(true);
    }
}

pub(crate) fn format_bytes(bytes: usize) -> String {
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
