use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::sync::Once;

const CPUROOT: &str = "/sys/devices/system/cpu";
const SYSTEM_HELPER: &str = "/usr/bin/cpufreqctl";
const SYSTEM_POLICY: &str = "/usr/share/polkit-1/actions/dev.skylar.cosmic-ext-applet-cpufreq.policy";

static INSTALL_CHECK: Once = Once::new();

/// Check if the helper and polkit policy are installed system-wide.
/// If not, run the install command via pkexec (one-time password prompt).
pub(crate) fn ensure_installed() {
    INSTALL_CHECK.call_once(|| {
        if !Path::new(SYSTEM_HELPER).exists() || !Path::new(SYSTEM_POLICY).exists() {
            tracing::info!("cpufreqctl not installed system-wide, running install...");
            let local = find_local_paths();
            if let Some((helper_path, policy_path)) = local {
                let status = Command::new("pkexec")
                    .arg(&helper_path)
                    .arg("install")
                    .arg(&helper_path)
                    .arg(&policy_path)
                    .status();
                match status {
                    Ok(s) if s.success() => {
                        tracing::info!("cpufreqctl installed successfully");
                    }
                    Ok(s) => {
                        tracing::error!("cpufreqctl install exited with: {s}");
                    }
                    Err(e) => {
                        tracing::error!("Failed to run cpufreqctl install: {e}");
                    }
                }
            } else {
                tracing::error!("Could not find local cpufreqctl/policy to bootstrap install");
            }
        }
    });
}

/// Find the local cpufreqctl helper and policy file (next to the binary or in ~/.local/bin).
fn find_local_paths() -> Option<(String, String)> {
    let policy_name = "dev.skylar.cosmic-ext-applet-cpufreq.policy";

    // Check next to the running binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let helper = dir.join("cpufreqctl");
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
        let helper = format!("{home}/.local/bin/cpufreqctl");
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
    let mut total: f64 = 0.0;
    let mut count: u32 = 0;

    let cpufreq_dir = fs::read_dir(format!("{CPUROOT}/cpufreq/")).ok()?;
    for entry in cpufreq_dir.flatten() {
        let path = entry.path().join("scaling_cur_freq");
        if let Ok(val) = fs::read_to_string(&path) {
            if let Ok(khz) = val.trim().parse::<f64>() {
                total += khz / 1000.0;
                count += 1;
            }
        }
    }

    if count > 0 {
        Some(total / f64::from(count))
    } else {
        None
    }
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
