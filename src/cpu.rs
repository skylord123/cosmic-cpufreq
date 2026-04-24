use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::Once;

const CPUROOT: &str = "/sys/devices/system/cpu";
const HELPER_NAME: &str = "cosmic-cpufreqctl";
const SYSTEM_HELPER: &str = "/usr/bin/cosmic-cpufreqctl";
const SYSTEM_POLICY: &str = "/usr/share/polkit-1/actions/dev.skylar.cosmic-ext-applet-cpufreq.policy";

static INSTALL_CHECK: Once = Once::new();

/// Check if the helper and polkit policy are installed system-wide.
/// If not, run the install command via pkexec (one-time password prompt).
pub(crate) fn ensure_installed() {
    INSTALL_CHECK.call_once(|| {
        let Some((helper_path, policy_path)) = find_local_paths() else {
            tracing::error!("Could not find local helper/policy to bootstrap install");
            return;
        };

        if !system_install_matches(&helper_path, SYSTEM_HELPER)
            || !system_install_matches(&policy_path, SYSTEM_POLICY)
        {
            tracing::info!("Helper or policy missing/stale, installing system-wide copy...");
            let status = Command::new("pkexec")
                .arg(&helper_path)
                .arg("install")
                .arg(&helper_path)
                .arg(&policy_path)
                .status();
            match status {
                Ok(s) if s.success() => {
                    tracing::info!("cpufreq helper installed successfully");
                }
                Ok(s) => {
                    tracing::error!("cpufreq helper install exited with: {s}");
                }
                Err(e) => {
                    tracing::error!("Failed to run helper install: {e}");
                }
            }
        }
    });
}

fn system_install_matches(local_path: &str, system_path: &str) -> bool {
    match (fs::read(local_path), fs::read(system_path)) {
        (Ok(local), Ok(system)) => local == system,
        _ => false,
    }
}

/// Find the local cpufreqctl helper and policy file (next to the binary or in ~/.local/bin).
fn find_local_paths() -> Option<(String, String)> {
    let policy_name = "dev.skylar.cosmic-ext-applet-cpufreq.policy";

    // Check next to the running binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let helper = dir.join(HELPER_NAME);
            let policy = dir.join(policy_name);
            if helper.exists() && policy.exists() {
                return Some((
                    helper.to_string_lossy().to_string(),
                    policy.to_string_lossy().to_string(),
                ));
            }
        }
    }
    // Check ~/.local/bin
    if let Ok(home) = std::env::var("HOME") {
        let helper = format!("{home}/.local/bin/{HELPER_NAME}");
        let policy = format!("{home}/.local/bin/{policy_name}");
        if Path::new(&helper).exists() && Path::new(&policy).exists() {
            return Some((helper, policy));
        }
    }
    None
}

/// Detect if intel_pstate driver is in use.
pub(crate) fn is_pstate() -> bool {
    fs::metadata(format!("{CPUROOT}/intel_pstate/no_turbo")).is_ok()
}

/// Read the current turbo boost state.
/// Returns `true` if turbo is enabled.
pub(crate) fn read_turbo_enabled() -> Option<bool> {
    if is_pstate() {
        // intel_pstate: no_turbo=0 means turbo ON
        let val = fs::read_to_string(format!("{CPUROOT}/intel_pstate/no_turbo")).ok()?;
        Some(val.trim() == "0")
    } else {
        // acpi/amd: boost=1 means turbo ON
        let val = fs::read_to_string(format!("{CPUROOT}/cpufreq/boost")).ok()?;
        Some(val.trim() == "1")
    }
}

/// Set turbo boost via helper script.
pub(crate) fn write_turbo(enabled: bool) -> io::Result<()> {
    if is_pstate() {
        let value = if enabled { "0" } else { "1" };
        run_helper(&["turbo-set", value])
    } else {
        let value = if enabled { "1" } else { "0" };
        run_helper(&["turbo-set", value])
    }
}

/// Read the average current CPU frequency across all cores in MHz.
pub(crate) fn read_current_frequency_mhz() -> Option<f64> {
    let cores = read_per_core_frequencies_mhz();
    if cores.is_empty() {
        return None;
    }
    let total: f64 = cores.iter().map(|(_, mhz)| mhz).sum();
    Some(total / cores.len() as f64)
}

/// Read per-core CPU frequencies in MHz, sorted by core ID.
pub(crate) fn read_per_core_frequencies_mhz() -> Vec<(u32, f64)> {
    let mut results = Vec::new();
    let Ok(entries) = fs::read_dir(CPUROOT) else {
        return results;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id_str) = name.strip_prefix("cpu") {
            if let Ok(id) = id_str.parse::<u32>() {
                let path = entry.path().join("cpufreq/scaling_cur_freq");
                if let Ok(val) = fs::read_to_string(&path) {
                    if let Ok(khz) = val.trim().parse::<f64>() {
                        results.push((id, khz / 1000.0));
                    }
                }
            }
        }
    }
    results.sort_by_key(|(id, _)| *id);
    results
}

/// Read the CPU model name from /proc/cpuinfo.
pub(crate) fn read_cpu_model() -> Option<String> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in cpuinfo.lines() {
        if let Some(val) = line.strip_prefix("model name") {
            let val = val.trim_start_matches(|c: char| c == ' ' || c == '\t' || c == ':');
            return Some(val.trim().to_string());
        }
    }
    None
}

/// Read machine vendor and product name from DMI.
pub(crate) fn read_machine_info() -> Option<(String, String)> {
    let vendor = fs::read_to_string("/sys/class/dmi/id/sys_vendor")
        .ok()
        .map(|s| s.trim().to_string())?;
    let model = fs::read_to_string("/sys/class/dmi/id/product_name")
        .ok()
        .map(|s| s.trim().to_string())?;
    Some((vendor, model))
}

/// Read the OS pretty name from /etc/os-release.
pub(crate) fn read_os_name() -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
            return Some(val.trim_matches('"').to_string());
        }
    }
    None
}

/// Read the kernel version.
pub(crate) fn read_kernel_version() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
}

/// Snapshot of CPU time counters from /proc/stat for computing usage %.
#[derive(Clone, Debug)]
pub(crate) struct CpuJiffies {
    pub(crate) user: u64,
    pub(crate) nice: u64,
    pub(crate) system: u64,
    pub(crate) idle: u64,
    pub(crate) iowait: u64,
    pub(crate) irq: u64,
    pub(crate) softirq: u64,
    pub(crate) steal: u64,
}

impl CpuJiffies {
    fn from_fields(vals: &[u64]) -> Option<Self> {
        if vals.len() < 8 {
            return None;
        }
        Some(Self {
            user: vals[0],
            nice: vals[1],
            system: vals[2],
            idle: vals[3],
            iowait: vals[4],
            irq: vals[5],
            softirq: vals[6],
            steal: vals[7],
        })
    }

    /// Compute CPU usage % between two snapshots.
    pub(crate) fn usage_percent(&self, prev: &Self) -> f64 {
        let busy = (self.user.wrapping_sub(prev.user)
            + self.nice.wrapping_sub(prev.nice)
            + self.system.wrapping_sub(prev.system)
            + self.irq.wrapping_sub(prev.irq)
            + self.softirq.wrapping_sub(prev.softirq)
            + self.steal.wrapping_sub(prev.steal)) as f64;
        let idle = (self.idle.wrapping_sub(prev.idle)
            + self.iowait.wrapping_sub(prev.iowait)) as f64;
        let total = busy + idle;
        if total > 0.0 {
            (busy / total) * 100.0
        } else {
            0.0
        }
    }
}

/// Snapshot of aggregate + per-core CPU usage from /proc/stat.
#[derive(Clone, Debug)]
pub(crate) struct CpuUsageSnapshot {
    pub(crate) aggregate: CpuJiffies,
    pub(crate) per_core: Vec<CpuJiffies>,
}

impl CpuUsageSnapshot {
    /// Compute aggregate CPU usage %.
    pub(crate) fn usage_percent(&self, prev: &Self) -> f64 {
        self.aggregate.usage_percent(&prev.aggregate)
    }

    /// Compute per-core usage % between two snapshots, indexed by core order.
    pub(crate) fn per_core_usage_percent(&self, prev: &Self) -> Vec<f64> {
        self.per_core
            .iter()
            .zip(prev.per_core.iter())
            .map(|(cur, prv)| cur.usage_percent(prv))
            .collect()
    }
}

/// Read aggregate + per-core CPU usage snapshot from /proc/stat.
pub(crate) fn read_cpu_usage_snapshot() -> Option<CpuUsageSnapshot> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let mut lines = stat.lines();

    // First line: aggregate "cpu  ..."
    let agg_line = lines.next()?;
    if !agg_line.starts_with("cpu ") {
        return None;
    }
    let agg_vals: Vec<u64> = agg_line[4..]
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let aggregate = CpuJiffies::from_fields(&agg_vals)?;

    // Per-core lines: "cpu0 ...", "cpu1 ...", etc.
    let mut per_core = Vec::new();
    for line in lines {
        if !line.starts_with("cpu") {
            break;
        }
        // Skip the "cpuN" prefix
        let rest = line.split_once(' ')?.1;
        let vals: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        if let Some(jiffies) = CpuJiffies::from_fields(&vals) {
            per_core.push(jiffies);
        }
    }

    Some(CpuUsageSnapshot {
        aggregate,
        per_core,
    })
}

/// Read memory usage: returns (used_kb, total_kb).
pub(crate) fn read_memory_usage() -> Option<(u64, u64)> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total: Option<u64> = None;
    let mut available: Option<u64> = None;
    for line in meminfo.lines() {
        if let Some(val) = line.strip_prefix("MemTotal:") {
            total = val.trim().strip_suffix("kB").and_then(|v| v.trim().parse().ok());
        } else if let Some(val) = line.strip_prefix("MemAvailable:") {
            available = val.trim().strip_suffix("kB").and_then(|v| v.trim().parse().ok());
        }
        if total.is_some() && available.is_some() {
            break;
        }
    }
    let total = total?;
    let available = available?;
    Some((total - available, total))
}

/// Read the available governors for cpu0.
pub(crate) fn read_available_governors() -> Vec<String> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/scaling_available_governors"))
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// Read the current governor for cpu0.
pub(crate) fn read_current_governor() -> Option<String> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/scaling_governor"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Set the governor on all CPUs via helper.
pub(crate) fn write_governor(governor: &str) -> io::Result<()> {
    run_helper(&["governor-set", governor])
}

/// Read available energy performance preferences.
pub(crate) fn read_available_epp() -> Vec<String> {
    fs::read_to_string(format!(
        "{CPUROOT}/cpu0/cpufreq/energy_performance_available_preferences"
    ))
    .unwrap_or_default()
    .split_whitespace()
    .map(String::from)
    .collect()
}

/// Read current energy performance preference.
pub(crate) fn read_current_epp() -> Option<String> {
    fs::read_to_string(format!(
        "{CPUROOT}/cpu0/cpufreq/energy_performance_preference"
    ))
    .ok()
    .map(|s| s.trim().to_string())
}

/// Set energy performance preference on all CPUs via helper.
pub(crate) fn write_epp(epp: &str) -> io::Result<()> {
    run_helper(&["epp-set", epp])
}

fn read_cpuinfo_max_frequency_mhz() -> Option<f64> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/cpuinfo_max_freq"))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .map(|khz| khz / 1000.0)
}

/// Read the non-turbo maximum frequency in MHz.
pub(crate) fn read_base_frequency_mhz() -> Option<f64> {
    // intel_pstate exposes the real non-turbo ceiling here.
    if let Ok(val) = fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/base_frequency"))
        && let Ok(khz) = val.trim().parse::<f64>()
    {
        return Some(khz / 1000.0);
    }

    read_cpuinfo_max_frequency_mhz()
}

/// Read the effective maximum configurable frequency in MHz.
///
/// This should match what the kernel will actually accept for scaling_max_freq.
pub(crate) fn read_effective_max_frequency_mhz() -> Option<f64> {
    if is_pstate() && !read_turbo_enabled().unwrap_or(true) {
        read_base_frequency_mhz()
    } else {
        read_cpuinfo_max_frequency_mhz()
    }
}

/// Read the hardware minimum frequency in MHz.
pub(crate) fn read_min_frequency_mhz() -> Option<f64> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/cpuinfo_min_freq"))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .map(|khz| khz / 1000.0)
}

/// Read current scaling min frequency in MHz.
pub(crate) fn read_scaling_min_mhz() -> Option<f64> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/scaling_min_freq"))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .map(|khz| khz / 1000.0)
}

/// Read current scaling max frequency in MHz.
pub(crate) fn read_scaling_max_mhz() -> Option<f64> {
    fs::read_to_string(format!("{CPUROOT}/cpu0/cpufreq/scaling_max_freq"))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .map(|khz| khz / 1000.0)
}

/// Set scaling min frequency on all CPUs (in kHz value) via helper.
pub(crate) fn write_scaling_min_khz(khz: u64) -> io::Result<()> {
    let bounded = clamp_frequency_khz(khz)?;
    run_helper(&["freq-min-set", &bounded.to_string()])
}

/// Set scaling max frequency on all CPUs (in kHz value) via helper.
pub(crate) fn write_scaling_max_khz(khz: u64) -> io::Result<()> {
    let bounded = clamp_frequency_khz(khz)?;
    run_helper(&["freq-max-set", &bounded.to_string()])
}

fn clamp_frequency_khz(khz: u64) -> io::Result<u64> {
    let min = read_min_frequency_mhz()
        .map(|mhz| (mhz * 1000.0) as u64)
        .ok_or_else(|| io::Error::other("missing minimum frequency bound"))?;
    let max = read_effective_max_frequency_mhz()
        .map(|mhz| (mhz * 1000.0) as u64)
        .ok_or_else(|| io::Error::other("missing maximum frequency bound"))?;

    Ok(khz.clamp(min, max))
}

/// Run the cpufreqctl helper via pkexec.
fn run_helper(args: &[&str]) -> io::Result<()> {
    ensure_installed();
    let output = Command::new("pkexec")
        .arg(SYSTEM_HELPER)
        .args(args)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("cpufreqctl failed: {stderr}");
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            stderr.to_string(),
        ))
    }
}
