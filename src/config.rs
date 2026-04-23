use cosmic::cosmic_config::{
    self, Config, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry,
};

const CONFIG_VERSION: u64 = 1;

pub(crate) const APP_ID: &str = "dev.skylar.cosmic-ext-applet-cpufreq";

#[derive(Debug, Clone, CosmicConfigEntry)]
pub(crate) struct CpuFreqConfig {
    /// Whether turbo boost should be enabled
    pub(crate) turbo_enabled: bool,
    /// Update interval in milliseconds for the frequency display
    pub(crate) update_interval_ms: u64,
}

impl Default for CpuFreqConfig {
    fn default() -> Self {
        Self {
            turbo_enabled: false,
            update_interval_ms: 2000,
        }
    }
}

impl CpuFreqConfig {
    fn config_handler() -> Option<Config> {
        Config::new(APP_ID, CONFIG_VERSION).ok()
    }

    fn config() -> CpuFreqConfig {
        match Self::config_handler() {
            Some(config_handler) => CpuFreqConfig::get_entry(&config_handler)
                .map_err(|error| {
                    tracing::info!("Error whilst loading config: {:#?}", error);
                })
                .unwrap_or_default(),
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
