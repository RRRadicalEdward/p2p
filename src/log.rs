use tracing_subscriber::{fmt, EnvFilter};

pub fn setup_logger() {
    let format = fmt::format()
        .with_ansi(true)
        .with_level(true)
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_source_location(true)
        .with_line_number(true)
        .pretty();

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("debug"))
        .add_directive("rustydht_lib=info".parse().unwrap());

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .event_format(format)
        .init();
}
