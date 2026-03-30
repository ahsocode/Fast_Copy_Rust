use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use rayon::prelude::*;
use walkdir::WalkDir;

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CopyMode {
    Auto,
    Large,
    Small,
}

impl Default for CopyMode {
    fn default() -> Self {
        CopyMode::Auto
    }
}

pub struct CopyJob {
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
    pub mode: CopyMode,
}

#[derive(Debug, Clone, Default)]
pub struct CopyProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub current_file: String,
    pub speed_bps: f64,
    pub elapsed_secs: f64,
    pub eta_secs: f64,
    pub errors: Vec<(String, String)>,
    pub finished: bool,
    pub cancelled: bool,
}

pub struct CopyEngine {
    cancel: Arc<AtomicBool>,
}

impl CopyEngine {
    pub fn new() -> Self {
        CopyEngine {
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, job: CopyJob) -> Receiver<CopyProgress> {
        let (tx, rx) = unbounded::<CopyProgress>();
        let cancel = self.cancel.clone();
        // Reset cancel flag
        cancel.store(false, Ordering::SeqCst);

        std::thread::spawn(move || {
            run_copy(job, tx, cancel);
        });

        rx
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

// ─── Speed tracker ───────────────────────────────────────────────────────────

struct SpeedTracker {
    window: VecDeque<(Instant, u64)>,
    window_duration: Duration,
}

impl SpeedTracker {
    fn new() -> Self {
        SpeedTracker {
            window: VecDeque::new(),
            window_duration: Duration::from_secs(2),
        }
    }

    fn add(&mut self, bytes: u64) {
        let now = Instant::now();
        self.window.push_back((now, bytes));
        // Prune old entries
        while let Some(front) = self.window.front() {
            if now.duration_since(front.0) > self.window_duration {
                self.window.pop_front();
            } else {
                break;
            }
        }
    }

    fn speed(&self) -> f64 {
        if self.window.len() < 2 {
            return 0.0;
        }
        let first = self.window.front().unwrap();
        let last = self.window.back().unwrap();
        let elapsed = last.0.duration_since(first.0).as_secs_f64();
        if elapsed <= 0.0 {
            return 0.0;
        }
        let total_bytes: u64 = self.window.iter().map(|(_, b)| b).sum();
        total_bytes as f64 / elapsed
    }
}

// ─── Background copy thread ───────────────────────────────────────────────────

fn run_copy(job: CopyJob, tx: Sender<CopyProgress>, cancel: Arc<AtomicBool>) {
    let start_time = Instant::now();

    // 1. Resolve existing sources
    let sources: Vec<PathBuf> = job
        .sources
        .iter()
        .filter(|p| p.exists())
        .cloned()
        .collect();

    if sources.is_empty() {
        let _ = tx.send(CopyProgress {
            finished: true,
            ..Default::default()
        });
        return;
    }

    // 2. Scan total bytes AND file count in a single pass (Bug 4 fix)
    let (bytes_total, files_total) = scan_sources(&sources);

    // 4. Create destination dir
    if let Err(e) = std::fs::create_dir_all(&job.destination) {
        let _ = tx.send(CopyProgress {
            finished: true,
            errors: vec![("destination".to_string(), e.to_string())],
            ..Default::default()
        });
        return;
    }

    // 5. Determine effective mode
    let effective_mode = match job.mode {
        CopyMode::Auto => {
            // Large only if single file > 100MB
            if sources.len() == 1 && sources[0].is_file() && bytes_total > 100 * 1024 * 1024 {
                CopyMode::Large
            } else {
                CopyMode::Small
            }
        }
        other => other,
    };

    let mut bytes_done: u64 = 0;
    let mut files_done: u64 = 0;
    let mut errors: Vec<(String, String)> = Vec::new();
    let mut speed_tracker = SpeedTracker::new();
    let mut last_send = Instant::now();

    let send_progress = |bytes_done: u64,
                         files_done: u64,
                         current_file: &str,
                         speed_bps: f64,
                         errors: &Vec<(String, String)>,
                         finished: bool,
                         cancelled: bool| {
        let elapsed = start_time.elapsed().as_secs_f64();
        let remaining = bytes_total.saturating_sub(bytes_done);
        let eta_secs = if speed_bps > 0.0 {
            remaining as f64 / speed_bps
        } else {
            0.0
        };
        let _ = tx.send(CopyProgress {
            bytes_done,
            bytes_total,
            files_done,
            files_total,
            current_file: current_file.to_string(),
            speed_bps,
            elapsed_secs: elapsed,
            eta_secs,
            errors: errors.clone(),
            finished,
            cancelled,
        });
    };

    match effective_mode {
        CopyMode::Large => {
            // Process each source as large file copy
            for source in &sources {
                if cancel.load(Ordering::SeqCst) {
                    send_progress(
                        bytes_done,
                        files_done,
                        "",
                        0.0,
                        &errors,
                        false,
                        true,
                    );
                    return;
                }

                let file_name = source
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                if source.is_dir() {
                    // For directories in Large mode, recurse
                    let file_pairs = collect_file_pairs(source, &job.destination);
                    for (src_file, dst_file) in &file_pairs {
                        if cancel.load(Ordering::SeqCst) {
                            send_progress(
                                bytes_done,
                                files_done,
                                "",
                                0.0,
                                &errors,
                                false,
                                true,
                            );
                            return;
                        }
                        if let Some(parent) = dst_file.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let prev_bytes = bytes_done;
                        let cur_file = src_file
                            .to_string_lossy()
                            .to_string();

                        match copy_large_file(
                            src_file,
                            dst_file,
                            &cancel,
                            &mut |chunk_bytes| {
                                bytes_done += chunk_bytes;
                                speed_tracker.add(chunk_bytes);
                            },
                        ) {
                            Ok(_) => {
                                files_done += 1;
                            }
                            Err(e) => {
                                errors.push((cur_file.clone(), e.to_string()));
                            }
                        }
                        let _ = prev_bytes; // suppress unused warning
                        if last_send.elapsed() >= Duration::from_millis(50) {
                            send_progress(
                                bytes_done,
                                files_done,
                                &cur_file,
                                speed_tracker.speed(),
                                &errors,
                                false,
                                false,
                            );
                            last_send = Instant::now();
                        }
                    }
                } else {
                    let dst_file = job.destination.join(&file_name);
                    let cur_file = source.to_string_lossy().to_string();

                    match copy_large_file(
                        source,
                        &dst_file,
                        &cancel,
                        &mut |chunk_bytes| {
                            bytes_done += chunk_bytes;
                            speed_tracker.add(chunk_bytes);
                        },
                    ) {
                        Ok(_) => {
                            files_done += 1;
                        }
                        Err(e) => {
                            errors.push((cur_file.clone(), e.to_string()));
                        }
                    }

                    if last_send.elapsed() >= Duration::from_millis(50) {
                        send_progress(
                            bytes_done,
                            files_done,
                            &cur_file,
                            speed_tracker.speed(),
                            &errors,
                            false,
                            false,
                        );
                        last_send = Instant::now();
                    }
                }
            }

            if cancel.load(Ordering::SeqCst) {
                send_progress(bytes_done, files_done, "", 0.0, &errors, false, true);
            } else {
                send_progress(
                    bytes_done,
                    files_done,
                    "",
                    speed_tracker.speed(),
                    &errors,
                    true,
                    false,
                );
            }
        }

        CopyMode::Small | CopyMode::Auto => {
            // Collect all file pairs
            let mut all_pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
            for source in &sources {
                if source.is_dir() {
                    all_pairs.extend(collect_file_pairs(source, &job.destination));
                } else {
                    let dst = job
                        .destination
                        .join(source.file_name().unwrap_or_default());
                    all_pairs.push((source.clone(), dst));
                }
            }

            // Ensure destination subdirs exist
            for (_, dst) in &all_pairs {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }

            copy_small_files(
                all_pairs,
                bytes_total,
                files_total,
                &tx,
                &cancel,
                start_time,
            );
        }
    }
}

// ─── Copy large file ─────────────────────────────────────────────────────────

fn copy_large_file(
    src: &Path,
    dst: &Path,
    cancel: &Arc<AtomicBool>,
    on_chunk: &mut dyn FnMut(u64),
) -> std::io::Result<()> {
    // On macOS, try clonefile first
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let src_cstr = std::ffi::CString::new(src.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let dst_cstr = std::ffi::CString::new(dst.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        // SYS_clonefile = 517 on macOS
        let ret = unsafe {
            libc::syscall(
                517, // SYS_clonefile
                src_cstr.as_ptr(),
                dst_cstr.as_ptr(),
                0i32,
            )
        };

        if ret == 0 {
            // clonefile succeeded; report file size
            if let Ok(meta) = src.metadata() {
                on_chunk(meta.len());
            }
            return Ok(());
        }
        // If clonefile failed (e.g. cross-volume), fall through to chunked copy
    }

    // Chunked copy with 64 MB chunks
    const CHUNK_SIZE: usize = 64 * 1024 * 1024;
    let mut src_file = std::fs::File::open(src)?;
    let mut dst_file = std::fs::File::create(dst)?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if cancel.load(Ordering::SeqCst) {
            // Clean up partial destination file
            drop(dst_file);
            let _ = std::fs::remove_file(dst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }

        let n = src_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst_file.write_all(&buf[..n])?;
        on_chunk(n as u64);
    }

    Ok(())
}

// ─── Copy small files (parallel) ─────────────────────────────────────────────

fn copy_small_files(
    pairs: Vec<(PathBuf, PathBuf)>,
    bytes_total: u64,
    files_total: u64,
    tx: &Sender<CopyProgress>,
    cancel: &Arc<AtomicBool>,
    start_time: Instant,
) {
    let bytes_done    = Arc::new(AtomicU64::new(0));
    let files_done    = Arc::new(AtomicU64::new(0));
    let current_file  = Arc::new(Mutex::new(String::new()));
    let errors        = Arc::new(Mutex::new(Vec::<(String, String)>::new()));

    // Bug 1 fix: explicit done flag so coordinator exits even when files_total == 0
    let rayon_done = Arc::new(AtomicBool::new(false));

    let bytes_done_c   = bytes_done.clone();
    let files_done_c   = files_done.clone();
    let current_file_c = current_file.clone();
    let errors_c       = errors.clone();
    let cancel_c       = cancel.clone();
    let rayon_done_c   = rayon_done.clone();
    let tx_c           = tx.clone();

    // Coordinator: emits progress snapshots at 20 Hz until rayon finishes or cancelled
    let coordinator = std::thread::spawn(move || {
        let mut speed_tracker = SpeedTracker::new();
        let mut prev_bytes: u64 = 0;

        loop {
            std::thread::sleep(Duration::from_millis(50));

            let bd    = bytes_done_c.load(Ordering::Relaxed);
            let fd    = files_done_c.load(Ordering::Relaxed);
            let cf    = current_file_c.lock().unwrap().clone();
            let errs  = errors_c.lock().unwrap().clone();
            let done  = rayon_done_c.load(Ordering::SeqCst);   // Bug 1 fix
            let cancelled = cancel_c.load(Ordering::SeqCst);

            let delta = bd.saturating_sub(prev_bytes);
            // Bug 2 fix: only add sample when bytes actually moved
            if delta > 0 {
                speed_tracker.add(delta);
            }
            prev_bytes = bd;

            let speed   = speed_tracker.speed();
            let elapsed = start_time.elapsed().as_secs_f64();
            let remaining = bytes_total.saturating_sub(bd);
            let eta = if speed > 0.0 { remaining as f64 / speed } else { 0.0 };

            let finished = done && !cancelled;

            let _ = tx_c.send(CopyProgress {
                bytes_done: bd,
                bytes_total,
                files_done: fd,
                files_total,
                current_file: cf,
                speed_bps: speed,
                elapsed_secs: elapsed,
                eta_secs: eta,
                errors: errs,
                finished,
                cancelled,
            });

            if finished || cancelled {
                break;
            }
        }
    });

    // Parallel copy using rayon
    pairs.par_iter().for_each(|(src, dst)| {
        if cancel.load(Ordering::SeqCst) {
            return;
        }

        let src_str   = src.to_string_lossy().to_string();
        let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

        if let Ok(mut cf) = current_file.lock() {
            *cf = src_str.clone();
        }

        let result = if file_size <= 4 * 1024 * 1024 {
            // Small file: std::fs::copy (OS-optimised — CopyFileEx/copy_file_range/clonefile)
            std::fs::copy(src, dst).map(|n| {
                bytes_done.fetch_add(n, Ordering::Relaxed);  // use actual bytes copied
            })
        } else {
            // Larger file: 1 MB buffered copy with per-chunk progress (Bug 3 fix)
            copy_buffered(src, dst, cancel, &bytes_done)
        };

        match result {
            Ok(_) => {
                files_done.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                if let Ok(mut errs) = errors.lock() {
                    errs.push((src_str, e.to_string()));
                }
                files_done.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    // Signal coordinator that rayon pool is done (Bug 1 fix)
    rayon_done.store(true, Ordering::SeqCst);
    let _ = coordinator.join();
}

/// 1 MB buffered copy with per-chunk progress reporting (Bug 3 fix).
fn copy_buffered(
    src: &Path,
    dst: &Path,
    cancel: &Arc<AtomicBool>,
    bytes_done: &Arc<AtomicU64>,    // updated after every chunk
) -> std::io::Result<()> {
    const BUF_SIZE: usize = 1024 * 1024; // 1 MB
    let mut src_file = std::fs::File::open(src)?;
    let mut dst_file = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; BUF_SIZE];

    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(dst_file);
            let _ = std::fs::remove_file(dst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let n = src_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst_file.write_all(&buf[..n])?;
        // Report bytes written after each chunk so coordinator can track live progress
        bytes_done.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Scan all sources in a single pass and return (total_bytes, total_files).
/// Bug 4 fix: replaces separate scan_total_size + count_total_files (double I/O).
pub fn scan_sources(sources: &[PathBuf]) -> (u64, u64) {
    let mut total_bytes: u64 = 0;
    let mut total_files: u64 = 0;
    for src in sources {
        if src.is_dir() {
            for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
                if entry.file_type().is_file() {
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    total_files += 1;
                }
            }
        } else if src.is_file() {
            total_bytes += src.metadata().map(|m| m.len()).unwrap_or(0);
            total_files += 1;
        }
    }
    (total_bytes, total_files)
}

/// Build (src_file, dst_file) pairs for all files under `src_dir`,
/// mirroring the directory structure under `dst_base`.
fn collect_file_pairs(src_dir: &Path, dst_base: &Path) -> Vec<(PathBuf, PathBuf)> {
    // Bug 5 fix: removed unused `parent` and `_rel` dead code
    let dst_root = dst_base.join(src_dir.file_name().unwrap_or_default());

    WalkDir::new(src_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let dst = dst_root.join(
                e.path().strip_prefix(src_dir).unwrap_or(e.path()),
            );
            (e.path().to_path_buf(), dst)
        })
        .collect()
}
