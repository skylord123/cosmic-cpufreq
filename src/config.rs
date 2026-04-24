use cosmic::cosmic_config::{
    self, Config, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry,
};

const CONFIG_VERSION: u64 = 1;

pub(crate) const APP_ID: &str = "dev.skylar.cosmic-ext-applet-cpufreq";

/// Which frequency value to display on the panel button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FreqDisplayMode {
    Average,
    Minimum,
    Maximum,
}

impl Default for FreqDisplayMode {
    fn default() -> Self {
        Self::Average
    }
}

impl std::fmt::Display for FreqDisplayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Average => write!(f, "Average"),
            Self::Minimum => write!(f, "Minimum"),
            Self::Maximum => write!(f, "Maximum"),
        }
    }
}

#[derive(
    Debug,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    CosmicConfigEntry,
)]
#[version = 1]
pub(crate) struct CpuFreqConfig {
    /// Whether turbo boost should be enabled
    pub(crate) turbo_enabled: bool,
    /// Update interval in milliseconds for the frequency display
    pub(crate) update_interval_ms: u64,
    /// Which frequency to show on the panel button
    pub(crate) freq_display_mode: FreqDisplayMode,
    /// Whether to show per-core CPU usage % in the core grid
    pub(crate) show_per_core_usage: bool,
    /// Whether to show the aggregate CPU usage bar
    pub(crate) show_cpu_usage: bool,
    /// Whether to show the memory usage bar
    pub(crate) show_memory_usage: bool,
    /// Whether to use horizontal layout (info left, controls right)
    pub(crate) horizontal_layout: bool,
}

impl Default for CpuFreqConfig {
    fn default() -> Self {
        Self {
            turbo_enabled: false,
            update_interval_ms: 2000,
            freq_display_mode: FreqDisplayMode::default(),
            show_per_core_usage: true,
            show_cpu_usage: true,
            show_memory_usage: true,
            horizontal_layout: true,
        }
    }
}

impl CpuFreqConfig {
    fn config_handler() -> Option<Config> {
        Config::new(APP_ID, CONFIG_VERSION).ok()
    }

    fn config() -> CpuFreqConfig {
        match Self::config_handler() {
            Some(config_handler) => match CpuFreqConfig::get_entry(&config_handler) {
                Ok(config) => config,
                Err((errors, partial)) => {
                    for error in errors {
                        tracing::info!("Config field missing or invalid: {error}");
                    }
                    partial
                }
            },
            None => CpuFreqConfig::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Flags {
    pub(crate) config: CpuFreqConfig,
    pub(crate) config_handler: Option<cosmic_config::Config>,
}

impl Flags {
    pub(crate) fn new() -> Self {
        Self {
            config: CpuFreqConfig::config(),
            config_handler: CpuFreqConfig::config_handler(),
        }
    }
}
