//! One shared CPU pool for the server and its download/browser/media process tree.
//! Affinity is inherited by new threads and children; periodically reconcile tools
//! that change their own affinity, and children created during a settings update.
use anyhow::{Context, Result};
use std::sync::Mutex;

pub struct CpuLimit {
    available: Vec<usize>,
    cores: Mutex<u16>,
}

pub fn supported() -> bool {
    cfg!(target_os = "linux")
}

pub fn default_cores() -> u16 {
    let available = available_cpus().map_or(1, |cpus| cpus.len().min(u16::MAX as usize) as u16);
    if supported() {
        available.min(2)
    } else {
        available
    }
}

impl CpuLimit {
    pub fn new() -> Result<Self> {
        let available = available_cpus().context("Could not determine available CPUs")?;
        anyhow::ensure!(!available.is_empty(), "No CPUs are available");
        let cores = Mutex::new(available.len().min(u16::MAX as usize) as u16);
        Ok(Self { available, cores })
    }

    pub fn available(&self) -> usize {
        self.available.len()
    }

    pub fn set(&self, count: u16) -> Result<()> {
        anyhow::ensure!(
            count > 0 && count as usize <= self.available.len(),
            "CPU core limit must be between 1 and {}",
            self.available.len()
        );
        anyhow::ensure!(
            supported() || count as usize == self.available.len(),
            "CPU core limits require Linux; use the Docker deployment"
        );
        let mut cores = self.cores.lock().unwrap();
        if let Err(error) = self.apply(count) {
            self.apply(*cores)
                .context("Could not restore the previous CPU limit")?;
            return Err(error);
        }
        *cores = count;
        Ok(())
    }

    pub fn reconcile(&self) -> Result<()> {
        let cores = self.cores.lock().unwrap();
        self.apply(*cores)
    }

    fn apply(&self, count: u16) -> Result<()> {
        // Use the same CPUs for every process, rather than N different CPUs per job.
        // Keep the original inventory so increasing the limit can enable CPUs again.
        let cpus = &self.available[self.available.len() - count as usize..];
        #[cfg(target_os = "linux")]
        for _ in 0..2 {
            apply_tree(std::process::id(), cpus)?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cpus;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn available_cpus() -> std::io::Result<Vec<usize>> {
    affinity(0)
}

#[cfg(not(target_os = "linux"))]
fn available_cpus() -> std::io::Result<Vec<usize>> {
    Ok((0..std::thread::available_parallelism()?.get()).collect())
}

#[cfg(target_os = "linux")]
fn affinity(tid: libc::pid_t) -> std::io::Result<Vec<usize>> {
    // SAFETY: cpu_set_t is a POD mask and the kernel receives its actual size.
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::sched_getaffinity(tid, std::mem::size_of_val(&mask), &mut mask) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((0..libc::CPU_SETSIZE as usize)
        .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &mask) })
        .collect())
}

#[cfg(target_os = "linux")]
fn apply_tree(root: u32, cpus: &[usize]) -> Result<()> {
    use std::{collections::HashSet, fs, io};
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for cpu in cpus {
        unsafe { libc::CPU_SET(*cpu, &mut mask) };
    }
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        let path = format!("/proc/{pid}/task");
        let threads = match fs::read_dir(&path) {
            Ok(threads) => threads,
            Err(error) if error.kind() == io::ErrorKind::NotFound && pid != root => continue,
            Err(error) => return Err(error).with_context(|| format!("Could not inspect {path}")),
        };
        for thread in threads {
            let thread = thread?;
            let Some(tid) = thread
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            let current = match affinity(tid) {
                Ok(current) => current,
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
                Err(error) => return Err(error).context("Could not read CPU affinity"),
            };
            if current != cpus {
                // Linux affinity is per-thread, not per-process. Set every thread.
                let result =
                    unsafe { libc::sched_setaffinity(tid, std::mem::size_of_val(&mask), &mask) };
                if result != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error).context("Could not apply CPU core limit");
                    }
                }
            }
            // Children can belong to any thread, including the PTY blocking thread.
            match fs::read_to_string(thread.path().join("children")) {
                Ok(children) => pending.extend(
                    children
                        .split_whitespace()
                        .filter_map(|s| s.parse::<u32>().ok()),
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("Could not inspect child processes"),
            }
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    // Run in a separate test process; never restrict the parallel test runner.
    #[test]
    fn live_process_tree_limit() {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "cpu::tests::isolated_process_tree",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    #[ignore = "invoked by live_process_tree_limit in isolation"]
    fn isolated_process_tree() {
        let limiter = CpuLimit::new().unwrap();
        let original = limiter.available.clone();
        let ready = tempfile::NamedTempFile::new().unwrap();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("cpu-test")
            .arg(ready.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let grandchild = (0..100)
            .find_map(|_| {
                let pid = std::fs::read_to_string(ready.path())
                    .unwrap()
                    .trim()
                    .parse::<i32>()
                    .ok();
                if pid.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                pid
            })
            .expect("grandchild started");
        // A thread existing before the change must be updated too.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            available_cpus().unwrap()
        });
        limiter.set(1).unwrap();
        let expected = &original[original.len() - 1..];
        assert_eq!(available_cpus().unwrap(), expected);
        assert_eq!(affinity(child.id() as i32).unwrap(), expected);
        assert_eq!(affinity(grandchild).unwrap(), expected);
        assert_eq!(
            std::thread::spawn(|| available_cpus().unwrap())
                .join()
                .unwrap(),
            expected
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(thread.join().unwrap(), expected);
        assert!(limiter.set(0).is_err());
        assert!(limiter.set(original.len() as u16 + 1).is_err());
        assert_eq!(affinity(grandchild).unwrap(), expected);
        limiter.set(original.len() as u16).unwrap();
        assert_eq!(available_cpus().unwrap(), original);
        assert_eq!(affinity(grandchild).unwrap(), original);
        // Reconcile a media tool that expands its own affinity.
        limiter.set(1).unwrap();
        let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        for cpu in &original {
            unsafe { libc::CPU_SET(*cpu, &mut mask) };
        }
        assert_eq!(
            unsafe { libc::sched_setaffinity(grandchild, std::mem::size_of_val(&mask), &mask) },
            0
        );
        limiter.reconcile().unwrap();
        assert_eq!(affinity(grandchild).unwrap(), expected);
        unsafe { libc::kill(grandchild, libc::SIGTERM) };
        child.wait().unwrap();

        // Saving must persist the chosen limit, and a failed database write must
        // restore the live affinity as well as leave the previous settings intact.
        use clap::Parser;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::parse_from([
            "pagecatch",
            "--output",
            dir.path().to_str().unwrap(),
        ]);
        let db = dir.path().join("tasks.sqlite");
        let store = Arc::new(crate::store::Store::open(&db).unwrap());
        let queue =
            crate::queue::Queue::new(Arc::new(config.clone()), store.clone(), Arc::new(limiter));
        let mut settings = config.download_settings();
        settings.cpu_cores = 1;
        queue.save_settings(settings.clone()).unwrap();
        assert_eq!(store.load_settings().unwrap().unwrap(), settings);
        rusqlite::Connection::open(&db).unwrap().execute_batch(
            "CREATE TRIGGER fail_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT, 'test write failure'); END;"
        ).unwrap();
        settings.cpu_cores = original.len() as u16;
        assert!(queue.save_settings(settings).is_err());
        assert_eq!(queue.current_config().cpu_cores, 1);
        assert_eq!(store.load_settings().unwrap().unwrap().cpu_cores, 1);
        assert_eq!(available_cpus().unwrap(), expected);
    }
}
