//! Material 3 Sound: a patchbay and mixer for PipeWire.
//!
//! `m3-sound` opens the window (`--demo` for a simulated desktop,
//! `--page <name>` to start on a page). `m3-sound --restore`
//! starts saved effects and reconnects the active scene without a window,
//! for running at login.

mod app;
mod backend;
mod config;
mod demo;
mod effects;
mod graph;
mod pwdump;
mod restore;
mod scenes;
mod ui;
mod units;
mod volume;

const HELP: &str = "Material 3 Sound: draw cords between apps, effects and devices.

Usage: m3-sound [options]

  --demo          Show a simulated desktop instead of your own sound
  --page <name>   Open on a page: patchbay, mixer, scenes, effects, settings
  --restore       Start saved effects and reconnect the active scene, then exit
  -h, --help      Show this help
  -V, --version   Show the version
";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut demo = false;
    let mut page = app::Page::Patchbay;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => demo = true,
            "--page" => {
                let name = it.next().map(String::as_str).unwrap_or_default();
                match app::Page::parse(name) {
                    Some(p) => page = p,
                    None => {
                        eprintln!("Unknown page `{name}`.\n\n{HELP}");
                        std::process::exit(2);
                    }
                }
            }
            "--restore" => std::process::exit(restore::run()),
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("m3-sound {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => {
                eprintln!("Unknown option `{other}`.\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    eframe::run_native(
        "Sound",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 800.0])
                .with_min_inner_size([760.0, 480.0])
                .with_app_id("material_3_sound")
                .with_title("Sound"),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(app::App::new(&cc.egui_ctx, demo, page)))),
    )
}
