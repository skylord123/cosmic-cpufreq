mod applet;
mod config;
mod cpu;
mod i18n;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!("Starting cpufreq applet");

    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    applet::run()
}
