//! Uso de CPU y RAM del sistema, sin dependencias.
//!
//! En Windows usa `GetSystemTimes` y `GlobalMemoryStatusEx` (kernel32); en el
//! resto lee `/proc`. El `Monitor` guarda la muestra anterior para calcular la
//! **diferencia** de CPU (el uso es un delta, no un valor absoluto).

/// Uso actual, en fracción `0.0..=1.0`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metrics {
    pub cpu: f32,
    pub ram: f32,
}

/// Muestrea CPU/RAM llevando el estado para el delta de CPU.
#[derive(Default)]
pub struct Monitor {
    prev: Option<(u64, u64)>,
}

impl Monitor {
    /// Nueva muestra. La primera vez, la CPU es 0 (no hay delta todavía).
    pub fn sample(&mut self) -> Metrics {
        let cpu = match cpu_times() {
            Some((idle, total)) => {
                let value = match self.prev {
                    Some((pi, pt)) => {
                        let di = idle.saturating_sub(pi);
                        let dt = total.saturating_sub(pt);
                        if dt > 0 {
                            (1.0 - di as f32 / dt as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        }
                    }
                    None => 0.0,
                };
                self.prev = Some((idle, total));
                value
            }
            None => 0.0,
        };
        Metrics {
            cpu,
            ram: mem_percent().unwrap_or(0.0),
        }
    }
}

#[cfg(windows)]
fn cpu_times() -> Option<(u64, u64)> {
    win::cpu_times()
}

#[cfg(windows)]
fn mem_percent() -> Option<f32> {
    win::mem_percent()
}

#[cfg(not(windows))]
fn cpu_times() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total: u64 = nums.iter().sum();
    Some((idle, total))
}

#[cfg(not(windows))]
fn mem_percent() -> Option<f32> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0f64;
    let mut avail = 0f64;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = v.split_whitespace().next()?.parse().ok()?;
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = v.split_whitespace().next()?.parse().ok()?;
        }
    }
    if total > 0.0 {
        Some(((total - avail) / total) as f32)
    } else {
        None
    }
}

#[cfg(windows)]
mod win {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn as_u64(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }

    pub fn cpu_times() -> Option<(u64, u64)> {
        let mut idle = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        // `GetSystemTimes` devuelve tiempos en unidades de 100 ns; el tiempo de
        // kernel incluye el idle, así que total = kernel + user.
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        let idle = idle.as_u64();
        let total = kernel.as_u64() + user.as_u64();
        Some((idle, total))
    }

    pub fn mem_percent() -> Option<f32> {
        let mut status = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            ..Default::default()
        };
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 {
            return None;
        }
        Some(status.memory_load as f32 / 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_devuelve_porcentajes_validos() {
        let mut monitor = Monitor::default();
        let _ = monitor.sample();
        let m = monitor.sample();
        assert!((0.0..=1.0).contains(&m.cpu));
        assert!((0.0..=1.0).contains(&m.ram));
    }
}
