//! System-wide CPU and memory load, shown in the menu bar popover's header.
//!
//! CPU load is the busy share of the processor time that passed between two samples, so the
//! first reading comes one sample after opening. Memory load is what Activity Monitor calls
//! "Memory Used" on macOS (app memory, wired and compressed) and total minus available elsewhere.

/// Cumulative processor time since boot, in the platform's own unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuTicks {
    busy: u64,
    total: u64,
}

/// Busy share (0–100) of the time between `previous` and `now`. `None` when no time passed or a
/// counter went backwards (macOS's 32-bit counters wrap after weeks of uptime).
pub fn cpu_percent(previous: CpuTicks, now: CpuTicks) -> Option<f32> {
    let total = now.total.checked_sub(previous.total).filter(|&t| t > 0)?;
    let busy = now.busy.checked_sub(previous.busy)?.min(total);
    Some(busy as f32 * 100. / total as f32)
}

/// Keeps the previous CPU sample and the latest readings.
pub struct Sampler {
    previous: Option<CpuTicks>,
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
}

impl Sampler {
    pub fn new() -> Self {
        Self { previous: cpu_ticks(), cpu: None, memory: memory_percent() }
    }

    pub fn sample(&mut self) {
        let now = cpu_ticks();
        if let (Some(previous), Some(now)) = (self.previous, now) {
            self.cpu = cpu_percent(previous, now).or(self.cpu);
        }
        self.previous = now.or(self.previous);
        self.memory = memory_percent().or(self.memory);
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc points to the mach2 crate for mach_host_self; the call is unchanged.
fn host() -> libc::mach_port_t {
    // Each call adds a reference to the host port; take it once.
    static HOST: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: mach_host_self has no preconditions.
    *HOST.get_or_init(|| unsafe { libc::mach_host_self() })
}

#[cfg(target_os = "macos")]
pub fn cpu_ticks() -> Option<CpuTicks> {
    let mut info = std::mem::MaybeUninit::<libc::host_cpu_load_info>::zeroed();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: the buffer is a host_cpu_load_info and `count` is its size in integers.
    let result = unsafe { libc::host_statistics(host(), libc::HOST_CPU_LOAD_INFO, info.as_mut_ptr().cast(), &mut count) };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: host_statistics succeeded; the struct started zeroed either way.
    let ticks = unsafe { info.assume_init() }.cpu_ticks;
    let total: u64 = ticks.iter().map(|&t| u64::from(t)).sum();
    Some(CpuTicks { busy: total - u64::from(ticks[libc::CPU_STATE_IDLE as usize]), total })
}

#[cfg(target_os = "macos")]
pub fn memory_percent() -> Option<f32> {
    let mut total: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: hw.memsize is a 64-bit integer and `size` says so.
    let result = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut total as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) };
    if result != 0 || total == 0 {
        return None;
    }
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the buffer is a vm_statistics64 and `count` is its size in integers; an older
    // kernel fills fewer fields and leaves the rest zeroed.
    let result = unsafe { libc::host_statistics64(host(), libc::HOST_VM_INFO64, stats.as_mut_ptr().cast(), &mut count) };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: host_statistics64 succeeded on a zeroed struct.
    let stats = unsafe { stats.assume_init() };
    // SAFETY: vm_page_size is set by the system before main runs.
    let page = unsafe { libc::vm_page_size } as u64;
    let app = u64::from(stats.internal_page_count).saturating_sub(u64::from(stats.purgeable_count));
    let used = (app + u64::from(stats.wire_count) + u64::from(stats.compressor_page_count)) * page;
    Some((used as f32 * 100. / total as f32).min(100.))
}

#[cfg(target_os = "linux")]
pub fn cpu_ticks() -> Option<CpuTicks> {
    parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

#[cfg(target_os = "linux")]
pub fn memory_percent() -> Option<f32> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// The aggregate `cpu` line of `/proc/stat`: user nice system idle iowait irq softirq steal …
/// (guest time is already counted in user and nice).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat(stat: &str) -> Option<CpuTicks> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line.split_whitespace().skip(1).take(8).map(|f| f.parse().ok()).collect::<Option<_>>()?;
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTicks { busy: total - idle, total })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_meminfo(meminfo: &str) -> Option<f32> {
    let field = |name: &str| -> Option<u64> {
        let line = meminfo.lines().find(|l| l.starts_with(name) && l[name.len()..].starts_with(':'))?;
        line[name.len() + 1..].split_whitespace().next()?.parse().ok()
    };
    let total = field("MemTotal").filter(|&t| t > 0)?;
    let available = field("MemAvailable")?.min(total);
    Some((total - available) as f32 * 100. / total as f32)
}

#[cfg(windows)]
pub fn cpu_ticks() -> Option<CpuTicks> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    let mut idle = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut kernel = idle;
    let mut user = idle;
    // SAFETY: three valid FILETIME out-pointers.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let value = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    // Kernel time includes idle time.
    let total = value(kernel) + value(user);
    Some(CpuTicks { busy: total.saturating_sub(value(idle)), total })
}

#[cfg(windows)]
pub fn memory_percent() -> Option<f32> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    // SAFETY: MEMORYSTATUSEX is plain data; zero is a valid value for every field.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a MEMORYSTATUSEX with dwLength set, as the call requires.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 || status.ullTotalPhys == 0 {
        return None;
    }
    let used = status.ullTotalPhys.saturating_sub(status.ullAvailPhys);
    Some(used as f32 * 100. / status.ullTotalPhys as f32)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn cpu_ticks() -> Option<CpuTicks> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn memory_percent() -> Option<f32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_is_the_busy_share_of_the_elapsed_time() {
        let previous = CpuTicks { busy: 100, total: 400 };
        assert_eq!(cpu_percent(previous, CpuTicks { busy: 125, total: 500 }), Some(25.));
        assert_eq!(cpu_percent(previous, previous), None);
        assert_eq!(cpu_percent(previous, CpuTicks { busy: 50, total: 300 }), None);
    }

    #[test]
    fn proc_stat_counts_iowait_as_idle() {
        let stat = "cpu  10 2 8 70 10 0 0 0 0 0\ncpu0 5 1 4 35 5 0 0 0 0 0\n";
        assert_eq!(parse_proc_stat(stat), Some(CpuTicks { busy: 20, total: 100 }));
        assert_eq!(parse_proc_stat("intr 1 2 3\n"), None);
    }

    #[test]
    fn meminfo_uses_available_memory() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    4000000 kB\n";
        assert_eq!(parse_meminfo(meminfo), Some(75.));
        assert_eq!(parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
    }

    #[test]
    fn this_machine_reports_memory_load() {
        if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            let percent = memory_percent().expect("memory load");
            assert!((0. ..=100.).contains(&percent), "{percent}");
            assert!(cpu_ticks().is_some());
        }
    }
}
