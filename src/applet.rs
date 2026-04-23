use std::time::Duration;

use tracing::{debug, trace};

use crate::{
    config::{APP_ID, CpuFreqConfig, Flags},
    cpu, fl,
};

pub(crate) fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<CpuFreqApplet>(Flags::new())
}

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
        };

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
                self.current_freq_mhz = cpu::read_current_frequency_mhz();
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
            }
            Message::ToggleWindow => {
                if let Some(id) = self.popup.take() {
                    return cosmic::iced::platform_specific::shell::commands::popup::destroy_popup(
                        id,
                    );
                }

                // Refresh static data when opening popup
                self.available_governors = cpu::read_available_governors();
                self.available_epp = cpu::read_available_epp();
                let min_freq = cpu::read_min_frequency_mhz();
                let max_freq = cpu::read_effective_max_frequency_mhz();
                self.freq_bounds = min_freq.zip(max_freq);

                let new_id = cosmic::iced::window::Id::unique();
                self.popup.replace(new_id);

                let popup_settings = self.core.applet.get_popup_settings(
                    self.core.main_window_id().unwrap(),
                    new_id,
                    None,
                    None,
                    None,
                );

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

        // Turbo boost toggle
        let turbo_row = cosmic::iced_widget::row![
            cosmic::widget::text(fl!("turbo-boost")),
            cosmic::widget::Space::new().width(cosmic::iced::Length::Fill),
            cosmic::widget::toggler(self.turbo_enabled.unwrap_or(false))
                .on_toggle(Message::SetTurbo),
        ];
        content = content.push(cosmic::applet::padded_control(turbo_row));

        // Governor dropdown
        if !self.available_governors.is_empty() {
            let current_idx = self
                .current_governor
                .as_ref()
                .and_then(|g| self.available_governors.iter().position(|x| x == g));

            let gov_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("cpu-governor")),
                cosmic::widget::dropdown(
                    &self.available_governors,
                    current_idx,
                    Message::SetGovernor,
                ),
            ]
            .spacing(4);
            content = content.push(cosmic::applet::padded_control(gov_row));
        }

        // EPP dropdown
        if !self.available_epp.is_empty() {
            let current_idx = self
                .current_epp
                .as_ref()
                .and_then(|e| self.available_epp.iter().position(|x| x == e));

            let epp_row = cosmic::iced_widget::column![
                cosmic::widget::text::body(fl!("energy-preference")),
                cosmic::widget::dropdown(&self.available_epp, current_idx, Message::SetEpp),
            ]
            .spacing(4);
            content = content.push(cosmic::applet::padded_control(epp_row));
        }

        // Frequency sliders
        if let Some((min_bound, max_bound)) = self.freq_bounds {
            let step = 100.0;

            // Use preview value while dragging, otherwise the committed value
            let display_min = self
                .preview_min_mhz
                .or(self.scaling_min_mhz);
            let display_max = self
                .preview_max_mhz
                .or(self.scaling_max_mhz);

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
                content = content.push(cosmic::applet::padded_control(min_row));
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
                content = content.push(cosmic::applet::padded_control(max_row));
            }
        }

        // Update interval input
        let interval_row = cosmic::iced_widget::column![
            cosmic::widget::text::body(fl!("update-interval")),
            cosmic::widget::text_input("2000", &self.update_interval_input)
                .on_input(Message::UpdateIntervalChanged),
        ]
        .spacing(4);
        content = content.push(cosmic::applet::padded_control(interval_row));

        let data = content.padding([16, 0]);

        self.core
            .applet
            .popup_container(cosmic::widget::container(data))
            .into()
    }
}

impl CpuFreqApplet {
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
}
