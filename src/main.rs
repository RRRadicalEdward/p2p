use anyhow::Context;
use log::error;
use p2p::{config::Config, service::NodeService};
use simple_logger::SimpleLogger;
use std::panic;
use tokio::runtime::Builder as RuntimeBuilder;

fn main() -> anyhow::Result<()> {
    SimpleLogger::new()
        .env()
        .init()
        .with_context(|| "logger initialization failed")?;

    panic::set_hook(Box::new(|panic_info| {
        error!(
            "{:?}:{}",
            panic_info.location(),
            panic_info.payload().downcast_ref::<String>().unwrap_or(&String::new()),
        );
    }));

    let config = Config::new();

    let mut node_service = NodeService::new(config);
    node_service.start();

    let rt = RuntimeBuilder::new_current_thread().enable_all().build()?;
    rt.block_on(build_signals_fut())?;

    node_service.stop();

    Ok(())
}

async fn build_signals_fut() -> anyhow::Result<()> {
    if cfg!(unix) {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate_signal = signal(SignalKind::terminate()).context("failed to create terminate signal")?;
        let mut quit_signal = signal(SignalKind::quit()).context("failed to create quit signal")?;
        let mut interrupt_signal = signal(SignalKind::interrupt()).context("failed to create interrupt signal")?;

        futures::future::select_all(vec![
            Box::pin(terminate_signal.recv()),
            Box::pin(quit_signal.recv()),
            Box::pin(interrupt_signal.recv()),
        ])
        .await;
    } else {
        tokio::signal::ctrl_c().await.context("CTRL_C signal failed")?;
    }

    println!("ctl + C");
    Ok(())
}
