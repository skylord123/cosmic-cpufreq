use std::time::Duration;

use tracing::{debug, trace};

use crate::{
    config::{APP_ID, CpuFreqConfig, Flags, FreqDisplayMode},
    cpu, fl,
};

pub(crate) fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<CpuFreqApplet>(Flags::new())
}

const DEFAULT_POPUP_WIDTH: f32 = 360.0;
const HORIZONTAL_POPUP_WIDTH: f32 = 600.0;

struct CpuFreqApplet {
    core: cosmic::app::Core,
    popup: Option<cosmic::iced::window::Id>,
    config: CpuFreqConfig,
    config_handler: Option<cosmic::cosmic_config::Config>,
    // Live state
    current_freq_mhz: Option<f64>,
    turbo_enabled: Option<bool>,
    current_governor: Option<String>,
    available_governors: Vec<String>,
    current_epp: Option<String>,
    available_epp: Vec<String>,
    /// (hw_min_mhz, turbo_max_mhz) - the full range including turbo
    freq_bounds: Option<(f64, f64)>,
    scaling_min_mhz: Option<f64>,
    scaling_max_mhz: Option<f64>,
    /// Preview values while dragging sliders (not yet committed to sysfs)
    preview_min_mhz: Option<f64>,
    preview_max_mhz: Option<f64>,
    update_interval_input: String,
    show_settings: bool,
    // System info (static, read once)
    cpu_model: Option<String>,
    machine_vendor: Option<String>,
    machine_model: Option<String>,
    os_name: Option<String>,
    kernel_version: Option<String>,
    // Per-core and usage (updated only when popup is open)
    per_core_freqs: Vec<(u32, f64)>,
    cpu_usage_prev: Option<cpu::CpuUsageSnapshot>,
    cpu_usage_percent: Option<f64>,
    per_core_usage: Vec<f64>,
    memory_used_kb: Option<u64>,
    memory_total_kb: Option<u64>,
    // Dropdown options stored to satisfy borrow lifetime
    display_mode_options: Vec<String>,
    layout_options: Vec<String>,
    governor_labels: Vec<String>,
    epp_labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Message {
    Tick,
    ToggleWindow,
    PopupClosed(cosmic::iced::window::Id),
    SetTurbo(bool),
    SetGovernor(usize),
    SetEpp(usize),
    /// Preview while dragging (UI only, no sysfs write)
    PreviewScalingMin(f64),
    PreviewScalingMax(f64),
    /// Commit on slider release
    CommitScalingMin,
    CommitScalingMax,
    UpdateIntervalChanged(String),
    ToggleSettings,
    SetFreqDisplayMode(usize),
    SetShowPerCoreUsage(bool),
    SetShowCpuUsage(bool),
    SetShowMemoryUsage(bool),
    SetLayout(usize),
}

impl cosmic::Application for CpuFreqApplet {
    type Flags = Flags;
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;

    const APP_ID: &'static str = APP_ID;

    fn init(
        core: cosmic::app::Core,
        flags: Self::Flags,
    ) -> (Self, cosmic::app::Task<Self::Message>) {
        let turbo_enabled = cpu::read_turbo_enabled();
        let current_governor = cpu::read_current_governor();
        let available_governors = cpu::read_available_governors();
        let current_epp = cpu::read_current_epp();
        let available_epp = cpu::read_available_epp();
        let min_freq = cpu::read_min_frequency_mhz();
        let max_freq = cpu::read_effective_max_frequency_mhz();
        let freq_bounds = min_freq.zip(max_freq);
        let scaling_min_mhz = cpu::read_scaling_min_mhz();
        let scaling_max_mhz = cpu::read_scaling_max_mhz();
        let current_freq_mhz = cpu::read_current_frequency_mhz();

        let update_interval_input = flags.config.update_interval_ms.to_string();

        let cpu_model = cpu::read_cpu_model();
        let (machine_vendor, machine_model) = cpu::read_machine_info()
            .map(|(v, m)| (Some(v), Some(m)))
            .unwrap_or((None, None));
        let os_name = cpu::read_os_name();
        let kernel_version = cpu::read_kernel_version();

        let mut applet = Self {
            core,
            popup: None,
            config: flags.config,
            config_handler: flags.config_handler,
            current_freq_mhz,
            turbo_enabled,
            current_governor,
            available_governors,
            current_epp,
            available_epp,
            freq_bounds,
            scaling_min_mhz,
            scaling_max_mhz,
            preview_min_mhz: None,
            preview_max_mhz: None,
            update_interval_input,
            show_settings: false,
            cpu_model,
            machine_vendor,
            machine_model,
            os_name,
            kernel_version,
            per_core_freqs: Vec::new(),
            cpu_usage_prev: None,
            cpu_usage_percent: None,
            per_core_usage: Vec::new(),
            memory_used_kb: None,
            memory_total_kb: None,
            display_mode_options: vec![
                fl!("display-average"),
                fl!("display-minimum"),
                fl!("display-maximum"),
            ],
            layout_options: vec![fl!("layout-horizontal"), fl!("layout-vertical")],
            governor_labels: Vec::new(),
            epp_labels: Vec::new(),
        };

        applet.rebuild_labels();

        // Restore saved turbo setting if the system state differs
        applet.restore_turbo_setting();

        (applet, cosmic::task::none())
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Message> {
        cosmic::iced::time::every(Duration::from_millis(self.config.update_interval_ms))
            .map(|_| Message::Tick)
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn on_close_requested(&self, id: cosmic::iced::window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Message) -> cosmic::app::Task<Self::Message> {
        match message {
            Message::Tick => trace!(?message),
            _ => debug!(?message),
        }

        match message {
            Message::Tick => {
                // Always read per-core so we can derive avg/min/max for panel
                self.per_core_freqs = cpu::read_per_core_frequencies_mhz();
                self.current_freq_mhz = if self.per_core_freqs.is_empty() {
                    None
                } else {
                    let freqs = self.per_core_freqs.iter().map(|(_, mhz)| *mhz);
                    match self.config.freq_display_mode {
                        FreqDisplayMode::Average => {
                            let total: f64 = freqs.clone().sum();
                            Some(total / self.per_core_freqs.len() as f64)
                        }
                        FreqDisplayMode::Minimum => freqs.reduce(f64::min),
                        FreqDisplayMode::Maximum => freqs.reduce(f64::max),
                    }
                };
                let system_turbo = cpu::read_turbo_enabled();
                // If turbo was changed externally, restore our saved setting
                if system_turbo != Some(self.config.turbo_enabled)
                    && self.turbo_enabled == Some(self.config.turbo_enabled)
                    && system_turbo != self.turbo_enabled
                {
                    tracing::info!(
                        "Turbo changed externally (now {:?}), restoring saved setting: {}",
                        system_turbo,
                        self.config.turbo_enabled
                    );
                    if cpu::write_turbo(self.config.turbo_enabled).is_ok() {
                        self.turbo_enabled = Some(self.config.turbo_enabled);
                    } else {
                        self.turbo_enabled = system_turbo;
                    }
                } else {
                    self.turbo_enabled = system_turbo;
                }
                self.current_governor = cpu::read_current_governor();
                self.current_epp = cpu::read_current_epp();
                // Only update from sysfs if not currently dragging a slider
                if self.preview_min_mhz.is_none() {
                    self.scaling_min_mhz = cpu::read_scaling_min_mhz();
                }
                if self.preview_max_mhz.is_none() {
                    self.scaling_max_mhz = cpu::read_scaling_max_mhz();
                }

                // Only poll usage data when popup is open
                if self.popup.is_some() {
                    if let Some(snapshot) = cpu::read_cpu_usage_snapshot() {
                        if let Some(prev) = &self.cpu_usage_prev {
                            self.cpu_usage_percent = Some(snapshot.usage_percent(prev));
                            self.per_core_usage = snapshot.per_core_usage_percent(prev);
                        }
                        self.cpu_usage_prev = Some(snapshot);
                    }

                    if let Some((used, total)) = cpu::read_memory_usage() {
                        self.memory_used_kb = Some(used);
                        self.memory_total_kb = Some(total);
                    }
                }
            }
            Message::ToggleWindow => {
                if let Some(id) = self.popup.take() {
                    return cosmic::iced::platform_specific::shell::commands::popup::destroy_popup(
                        id,
                    );
                }

                // Refresh data when opening popup
                self.show_settings = false;
                self.available_governors = cpu::read_available_governors();
                self.available_epp = cpu::read_available_epp();
                self.rebuild_labels();
                let min_freq = cpu::read_min_frequency_mhz();
                let max_freq = cpu::read_effective_max_frequency_mhz();
                self.freq_bounds = min_freq.zip(max_freq);
                self.per_core_freqs = cpu::read_per_core_frequencies_mhz();
                self.cpu_usage_prev = cpu::read_cpu_usage_snapshot();
                self.cpu_usage_percent = None; // need two snapshots
                if let Some((used, total)) = cpu::read_memory_usage() {
                    self.memory_used_kb = Some(used);
                    self.memory_total_kb = Some(total);
                }

                let new_id = cosmic::iced::window::Id::unique();
                self.popup.replace(new_id);

                let popup_width = self.popup_width() as u32;
                let mut popup_settings = self.core.applet.get_popup_settings(
                    self.core.main_window_id().unwrap(),
                    new_id,
                    Some((popup_width, 1)),
                    None,
                    None,
                );
                popup_settings.positioner.size_limits = self.popup_limits();

                return cosmic::iced::platform_specific::shell::commands::popup::get_popup(
                    popup_settings,
                );
            }
            Message::PopupClosed(id) => {
                self.popup.take_if(|stored_id| stored_id == &id);
            }
            Message::SetTurbo(enabled) => {
                if let Err(e) = cpu::write_turbo(enabled) {
                    tracing::error!("Failed to set turbo: {e}");
                } else {
                    self.turbo_enabled = Some(enabled);
                    let min_freq = cpu::read_min_frequency_mhz();
                    let max_freq = cpu::read_effective_max_frequency_mhz();
                    self.freq_bounds = min_freq.zip(max_freq);
                    self.scaling_min_mhz = cpu::read_scaling_min_mhz();
                    self.scaling_max_mhz = cpu::read_scaling_max_mhz();
                    if let Some(handler) = &self.config_handler
                        && let Err(error) = self.config.set_turbo_enabled(handler, enabled)
                    {
                        tracing::error!("Failed to save turbo config: {error}");
                    }
                }
            }
            Message::SetGovernor(idx) => {
                if let Some(gov) = self.available_governors.get(idx) {
                    if let Err(e) = cpu::write_governor(gov) {
                        tracing::error!("Failed to set governor: {e}");
                    } else {
                        self.current_governor = Some(gov.clone());
                    }
                }
            }
            Message::SetEpp(idx) => {
                if let Some(epp) = self.available_epp.get(idx) {
                    if let Err(e) = cpu::write_epp(epp) {
                        tracing::error!("Failed to set EPP: {e}");
                    } else {
                        self.current_epp = Some(epp.clone());
                    }
                }
            }
            Message::PreviewScalingMin(mhz) => {
                self.preview_min_mhz = Some(mhz);
            }
            Message::PreviewScalingMax(mhz) => {
                self.preview_max_mhz = Some(mhz);
            }
            Message::CommitScalingMin => {
                if let Some(mhz) = self.preview_min_mhz.take() {
                    let khz = (mhz * 1000.0) as u64;
                    if let Err(e) = cpu::write_scaling_min_khz(khz) {
                        tracing::error!("Failed to set min freq: {e}");
                    } else {
                        self.scaling_min_mhz = Some(mhz);
                    }
                }
            }
            Message::CommitScalingMax => {
                if let Some(mhz) = self.preview_max_mhz.take() {
                    let khz = (mhz * 1000.0) as u64;
                    if let Err(e) = cpu::write_scaling_max_khz(khz) {
                        tracing::error!("Failed to set max freq: {e}");
                    } else {
                        self.scaling_max_mhz = Some(mhz);
                    }
                }
            }
            Message::UpdateIntervalChanged(value) => {
                self.update_interval_input = value.clone();
                if let Ok(ms) = value.parse::<u64>() {
                    if ms >= 100 {
                        if let Some(handler) = &self.config_handler
                            && let Err(error) =
                                self.config.set_update_interval_ms(handler, ms)
                        {
                            tracing::error!("Failed to save update interval: {error}");
                        }
                    }
                }
            }
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
            }
            Message::SetFreqDisplayMode(idx) => {
                let mode = match idx {
                    1 => FreqDisplayMode::Minimum,
                    2 => FreqDisplayMode::Maximum,
                    _ => FreqDisplayMode::Average,
                };
                if let Some(handler) = &self.config_handler
                    && let Err(error) = self.config.set_freq_display_mode(handler, mode)
                {
                    tracing::error!("Failed to save display mode: {error}");
                }
            }
            Message::SetShowPerCoreUsage(enabled) => {
                if let Some(handler) = &self.config_handler
                    && let Err(error) =
                        self.config.set_show_per_core_usage(handler, enabled)
                {
                    tracing::error!("Failed to save per-core usage setting: {error}");
                }
            }
            Message::SetShowCpuUsage(enabled) => {
                if let Some(handler) = &self.config_handler
                    && let Err(error) = self.config.set_show_cpu_usage(handler, enabled)
                {
                    tracing::error!("Failed to save CPU usage setting: {error}");
                }
            }
            Message::SetShowMemoryUsage(enabled) => {
                if let Some(handler) = &self.config_handler
                    && let Err(error) = self.config.set_show_memory_usage(handler, enabled)
                {
                    tracing::error!("Failed to save memory usage setting: {error}");
                }
            }
            Message::SetLayout(idx) => {
                let enabled = idx == 0;
                if let Some(handler) = &self.config_handler
                    && let Err(error) = self.config.set_horizontal_layout(handler, enabled)
                {
                    tracing::error!("Failed to save layout setting: {error}");
                }
            }
        }

        cosmic::task::none()
    }

    fn view(&self) -> cosmic::Element<'_, Message> {
        let freq_text = match self.current_freq_mhz {
            Some(mhz) if mhz >= 1000.0 => format!("{:.2} GHz", mhz / 1000.0),
            Some(mhz) => format!("{:.0} MHz", mhz),
            None => "--".to_string(),
        };

        let content = cosmic::widget::text(freq_text).size(12);

        let button = cosmic::widget::button::custom(content)
            .class(cosmic::theme::Button::AppletIcon)
            .on_press_down(Message::ToggleWindow);

        cosmic::widget::autosize::autosize(button, cosmic::widget::Id::unique()).into()
    }

    fn view_window(&self, _id: cosmic::iced::window::Id) -> cosmic::Element<'_, Message> {
        let mut content = cosmic::iced_widget::column![].spacing(8);

        if self.show_settings {
            // ── Settings page ──
            let header = cosmic::iced_widget::row![
                cosmic::widget::button::icon(
                    cosmic::widget::icon::from_name("go-previous-symbolic")
                )
                .on_press(Message::ToggleSettings),
                cosmic::widget::text::title4(fl!("settings")),
            ]
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);
            content = content.push(cosmic::applet::padded_control(header));

            content = content.push(cosmic::applet::padded_control(
                cosmic::widget::divider::horizontal::default(),
            ));

            // Display mode dropdown
            let current_idx = match self.config.freq_display_mode {
                FreqDisplayMode::Average => Some(0),
                FreqDisplayMode::Minimum => Some(1),
                FreqDisplayMode::Maximum => Some(2),
            };
            let mode_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("display-mode")),
                cosmic::widget::dropdown(
                    &self.display_mode_options,
                    current_idx,
                    Message::SetFreqDisplayMode,
                ),
            ]
            .spacing(4);
            content = content.push(cosmic::applet::padded_control(mode_row));

            // Per-core usage toggle
            let usage_row = cosmic::iced_widget::row![
                cosmic::widget::text(fl!("show-per-core-usage")),
                cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                cosmic::widget::toggler(self.config.show_per_core_usage)
                    .on_toggle(Message::SetShowPerCoreUsage),
            ];
            content = content.push(cosmic::applet::padded_control(usage_row));

            // Show CPU usage toggle
            let cpu_usage_row = cosmic::iced_widget::row![
                cosmic::widget::text(fl!("show-cpu-usage")),
                cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                cosmic::widget::toggler(self.config.show_cpu_usage)
                    .on_toggle(Message::SetShowCpuUsage),
            ];
            content = content.push(cosmic::applet::padded_control(cpu_usage_row));

            // Show memory usage toggle
            let mem_usage_row = cosmic::iced_widget::row![
                cosmic::widget::text(fl!("show-memory-usage")),
                cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                cosmic::widget::toggler(self.config.show_memory_usage)
                    .on_toggle(Message::SetShowMemoryUsage),
            ];
            content = content.push(cosmic::applet::padded_control(mem_usage_row));

            let current_layout_idx = Some(if self.config.horizontal_layout { 0 } else { 1 });
            let layout_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("layout")),
                cosmic::widget::dropdown(
                    &self.layout_options,
                    current_layout_idx,
                    Message::SetLayout,
                ),
            ];
            let layout_row = layout_row.spacing(4);
            content = content.push(cosmic::applet::padded_control(layout_row));

            // Update interval input
            let interval_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("update-interval")),
                cosmic::widget::text_input("2000", &self.update_interval_input)
                    .on_input(Message::UpdateIntervalChanged),
            ]
            .spacing(4);
            content = content.push(cosmic::applet::padded_control(interval_row));
        } else {
            // ── Main popup page ──

            // System info header with settings cog (always at top)
            let mut info_col = cosmic::iced_widget::column![].spacing(2);
            if let Some(model) = &self.cpu_model {
                info_col = info_col.push(cosmic::widget::text::body(model.clone()));
            }
            if let (Some(vendor), Some(model)) = (&self.machine_vendor, &self.machine_model) {
                info_col = info_col.push(
                    cosmic::widget::text::caption(format!("{vendor} {model}")),
                );
            }
            if let Some(os) = &self.os_name {
                info_col = info_col.push(cosmic::widget::text::caption(os.clone()));
            }
            if let Some(kern) = &self.kernel_version {
                info_col = info_col.push(
                    cosmic::widget::text::caption(format!("{} {kern}", fl!("kernel"))),
                );
            }

            let header_row = cosmic::iced_widget::row![
                info_col,
                cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                cosmic::widget::button::icon(
                    cosmic::widget::icon::from_name("emblem-system-symbolic")
                )
                .on_press(Message::ToggleSettings),
            ]
            .align_y(cosmic::iced::Alignment::Start);
            content = content.push(cosmic::applet::padded_control(header_row));

            let info_section = self.build_info_section();
            let controls_section = self.build_controls_section();

            if self.config.horizontal_layout {
                let row = cosmic::iced_widget::row![
                    cosmic::widget::container(info_section)
                        .width(cosmic::iced::Length::FillPortion(1)),
                    cosmic::widget::container(controls_section)
                        .width(cosmic::iced::Length::FillPortion(1)),
                ]
                .spacing(16);
                content = content.push(row);
            } else {
                content = content.push(info_section);
                content = content.push(cosmic::applet::padded_control(
                    cosmic::widget::divider::horizontal::default(),
                ));
                content = content.push(controls_section);
            }
        }

        let data = content.padding([16, 0]);

        let container = if !self.show_settings && self.config.horizontal_layout {
            cosmic::widget::container(data)
                .width(cosmic::iced::Length::Fixed(600.0))
        } else {
            cosmic::widget::container(data)
        };

        self.core
            .applet
            .popup_container(container)
            .limits(self.popup_limits())
            .into()
    }
}

impl CpuFreqApplet {
    fn popup_width(&self) -> f32 {
        if !self.show_settings && self.config.horizontal_layout {
            HORIZONTAL_POPUP_WIDTH
        } else {
            DEFAULT_POPUP_WIDTH
        }
    }

    fn popup_limits(&self) -> cosmic::iced::Limits {
        let width = self.popup_width();
        cosmic::iced::Limits::NONE
            .min_height(1.0)
            .min_width(width)
            .max_width(width)
            .max_height(1080.0)
    }

    /// Restore saved turbo setting if system state differs from config.
    fn restore_turbo_setting(&mut self) {
        if let Some(system_turbo) = self.turbo_enabled {
            if system_turbo != self.config.turbo_enabled {
                tracing::info!(
                    "Restoring turbo setting: {} (system was: {})",
                    self.config.turbo_enabled,
                    system_turbo
                );
                if cpu::write_turbo(self.config.turbo_enabled).is_ok() {
                    self.turbo_enabled = Some(self.config.turbo_enabled);
                }
            }
        }
    }

    /// Rebuild pretty display labels for governor and EPP dropdowns.
    fn rebuild_labels(&mut self) {
        self.governor_labels = self
            .available_governors
            .iter()
            .map(|s| prettify_label(s))
            .collect();
        self.epp_labels = self
            .available_epp
            .iter()
            .map(|s| prettify_label(s))
            .collect();
    }

    /// Build the info section (per-core grid, usage bars).
    fn build_info_section(&self) -> cosmic::Element<'_, Message> {
        let mut col = cosmic::iced_widget::column![].spacing(8);

        // Per-core frequency grid
        if !self.per_core_freqs.is_empty() {
            let cols = 4usize;
            let mut grid_col = cosmic::iced_widget::column![].spacing(6);

            for (chunk_idx, chunk) in self.per_core_freqs.chunks(cols).enumerate() {
                let mut row = cosmic::iced_widget::row![].spacing(8);
                for (i, (id, mhz)) in chunk.iter().enumerate() {
                    let freq_str = if *mhz >= 1000.0 {
                        format!("{:.2} GHz", mhz / 1000.0)
                    } else {
                        format!("{:.0} MHz", mhz)
                    };
                    let mut cell = cosmic::iced_widget::column![
                        cosmic::widget::text(format!("CPU{id}")).size(11),
                        cosmic::widget::text(freq_str).size(13),
                    ]
                    .align_x(cosmic::iced::Alignment::Center)
                    .width(cosmic::iced::Length::FillPortion(1));

                    if self.config.show_per_core_usage {
                        let core_idx = chunk_idx * cols + i;
                        let usage_str = self
                            .per_core_usage
                            .get(core_idx)
                            .map(|pct| format!("{pct:.0}%"))
                            .unwrap_or_else(|| "--".to_string());
                        cell = cell.push(cosmic::widget::text(usage_str).size(10));
                    }

                    row = row.push(cell);
                }
                for _ in chunk.len()..cols {
                    row = row.push(
                        cosmic::widget::Space::new()
                            .width(cosmic::iced::Length::FillPortion(1)),
                    );
                }
                grid_col = grid_col.push(row);
            }
            col = col.push(cosmic::applet::padded_control(grid_col));
        }

        // CPU usage bar
        if self.config.show_cpu_usage {
            let pct = self.cpu_usage_percent.unwrap_or(0.0);
            let bar_row = cosmic::iced_widget::column![
                cosmic::iced_widget::row![
                    cosmic::widget::text::body(fl!("cpu-usage")),
                    cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                    cosmic::widget::text::body(format!("{pct:.1}%")),
                ]
                .align_y(cosmic::iced::Alignment::Center),
                cosmic::widget::progress_bar(0.0..=100.0, pct as f32),
            ]
            .spacing(4);
            col = col.push(cosmic::applet::padded_control(bar_row));
        }

        // Memory usage bar
        if self.config.show_memory_usage {
            let (pct, label) = match (self.memory_used_kb, self.memory_total_kb) {
                (Some(used), Some(total)) if total > 0 => {
                    let p = (used as f64 / total as f64) * 100.0;
                    (p, format!("{p:.1}%"))
                }
                _ => (0.0, "--".to_string()),
            };
            let bar_row = cosmic::iced_widget::column![
                cosmic::iced_widget::row![
                    cosmic::widget::text::body(fl!("memory-usage")),
                    cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
                    cosmic::widget::text::body(label),
                ]
                .align_y(cosmic::iced::Alignment::Center),
                cosmic::widget::progress_bar(0.0..=100.0, pct as f32),
            ]
            .spacing(4);
            col = col.push(cosmic::applet::padded_control(bar_row));
        }

        col.into()
    }

    /// Build the controls section (turbo, governor, EPP, freq sliders).
    fn build_controls_section(&self) -> cosmic::Element<'_, Message> {
        let mut col = cosmic::iced_widget::column![].spacing(8);

        // Turbo boost toggle
        let turbo_row = cosmic::iced_widget::row![
            cosmic::widget::text(fl!("turbo-boost")),
            cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
            cosmic::widget::toggler(self.turbo_enabled.unwrap_or(false))
                .on_toggle(Message::SetTurbo),
        ];
        col = col.push(cosmic::applet::padded_control(turbo_row));

        // Governor dropdown (pretty labels)
        if !self.available_governors.is_empty() {
            let current_idx = self
                .current_governor
                .as_ref()
                .and_then(|g| self.available_governors.iter().position(|x| x == g));

            let gov_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("cpu-governor")),
                cosmic::widget::dropdown(
                    &self.governor_labels,
                    current_idx,
                    Message::SetGovernor,
                ),
            ]
            .spacing(4);
            col = col.push(cosmic::applet::padded_control(gov_row));
        }

        // EPP dropdown (pretty labels)
        if !self.available_epp.is_empty() {
            let current_idx = self
                .current_epp
                .as_ref()
                .and_then(|e| self.available_epp.iter().position(|x| x == e));

            let epp_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("energy-preference")),
                cosmic::widget::dropdown(&self.epp_labels, current_idx, Message::SetEpp),
            ]
            .spacing(4);
            col = col.push(cosmic::applet::padded_control(epp_row));
        }

        // Frequency sliders
        if let Some((min_bound, max_bound)) = self.freq_bounds {
            let step = 100.0;

            let display_min = self.preview_min_mhz.or(self.scaling_min_mhz);
            let display_max = self.preview_max_mhz.or(self.scaling_max_mhz);

            if let Some(scaling_min) = display_min {
                let min_label = if scaling_min >= 1000.0 {
                    format!("{}: {:.1} GHz", fl!("min-frequency"), scaling_min / 1000.0)
                } else {
                    format!("{}: {:.0} MHz", fl!("min-frequency"), scaling_min)
                };
                let min_row = cosmic::iced_widget::column![
                    cosmic::widget::text::body(min_label),
                    cosmic::widget::slider(min_bound..=max_bound, scaling_min, move |v| {
                        Message::PreviewScalingMin((v / step).round() * step)
                    })
                    .on_release(Message::CommitScalingMin),
                ]
                .spacing(4);
                col = col.push(cosmic::applet::padded_control(min_row));
            }

            if let Some(scaling_max) = display_max {
                let max_label = if scaling_max >= 1000.0 {
                    format!("{}: {:.1} GHz", fl!("max-frequency"), scaling_max / 1000.0)
                } else {
                    format!("{}: {:.0} MHz", fl!("max-frequency"), scaling_max)
                };
                let max_row = cosmic::iced_widget::column![
                    cosmic::widget::text::body(max_label),
                    cosmic::widget::slider(min_bound..=max_bound, scaling_max, move |v| {
                        Message::PreviewScalingMax((v / step).round() * step)
                    })
                    .on_release(Message::CommitScalingMax),
                ]
                .spacing(4);
                col = col.push(cosmic::applet::padded_control(max_row));
            }
        }

        col.into()
    }
}

/// Convert a sysfs-style label like "balance_performance" to "Balance Performance".
fn prettify_label(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{upper}{}", chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
