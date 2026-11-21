//! Obsidia - flash and restore iPhone/iPad firmware.
//!
//! Starts the GUI without arguments, otherwise runs the CLI.

// no console window for release builds on windows
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cli;
mod device;
mod diagnostics;
mod drivers;
mod firmware;
mod gui;
mod guides;
mod icons;
mod settings;
mod theme;
mod updater;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "obsidia",
    version,
    about = "Obsidia - iPhone/iPad firmware flashing tool"
)]
struct Args {
    /// UDID or ECID of the target device (needed if several are connected)
    #[arg(short, long, global = true)]
    udid: Option<String>,

    #[command(subcommand)]
    command: Option<cli::Cmd>,
}

/// Reattach to the parent console so CLI output shows up in the terminal.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: plain Win32 call without pointers; failure is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        attach_console();
    }

    let args = Args::parse();
    let result = match args.command {
        None | Some(cli::Cmd::Gui) => gui::run(),
        Some(cmd) => cli::run(cmd, args.udid.as_deref()),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
