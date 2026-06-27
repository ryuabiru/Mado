#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod cli;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::Cli;
use mado::config::Config;
use mado::nvim::process::{NvimLaunchOptions, NvimProcess, discover_nvim};
use mado::ui::app::MadoApp;
use tracing::info;
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref());
    let executable = discover_nvim().context("Neovim executable was not found")?;

    info!(path = %executable.display(), "starting Neovim");
    let nvim = NvimProcess::spawn(NvimLaunchOptions {
        executable,
        files: cli.files,
        clean: false,
    })?;
    let api_info = nvim
        .rpc()
        .request("nvim_get_api_info", vec![])
        .context("nvim_get_api_info failed")?;
    ensure_linegrid_support(&api_info)?;
    nvim.attach_ui(80, 24).context("nvim_ui_attach failed")?;

    let event_loop = EventLoop::new().context("failed to create event loop")?;
    mado::platform::install_file_open_handler();
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
    event_loop
        .run_app(&mut MadoApp::new(nvim, config))
        .context("Mado event loop failed")
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("mado=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn ensure_linegrid_support(api_info: &rmpv::Value) -> Result<()> {
    let metadata = api_info
        .as_array()
        .and_then(|values| values.get(1))
        .and_then(rmpv::Value::as_map)
        .context("nvim_get_api_info returned invalid metadata")?;
    let supports_linegrid = metadata.iter().any(|(key, value)| {
        key.as_str() == Some("ui_options")
            && value.as_array().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.as_str() == Some("ext_linegrid"))
            })
    });
    if supports_linegrid {
        Ok(())
    } else {
        bail!("this Neovim does not advertise ext_linegrid support")
    }
}
