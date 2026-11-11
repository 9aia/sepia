//! `tamias-driver-sepia-config` — `type: sepia-config` content
//! driver binary. Dispatched by tamias as
//! `tamias-driver-sepia-config <verb> --dir <payload> --name <pack>`.

use sepia_driver_config as drv;

const USAGE: &str = "usage: tamias-driver-sepia-config <verb> --dir <payload> --name <pack> [--<agent>-dir <path>] [--dry-run]\n       verbs: install | deploy | uninstall | config-schema | capabilities";

fn main() -> std::process::ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(verb) = argv.first().cloned() else {
        eprintln!("{USAGE}");
        return std::process::ExitCode::from(2);
    };
    argv.remove(0);
    let result = match verb.as_str() {
        "install" | "deploy" | "uninstall" => {
            drv::parse_args(&argv).and_then(|a| match verb.as_str() {
                "install" => drv::install(&a),
                "deploy" => drv::deploy(&a),
                _ => drv::uninstall(&a),
            })
        }
        "config-schema" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&drv::config_schema()).unwrap_or_default()
            );
            Ok(())
        }
        "capabilities" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&drv::capabilities()).unwrap_or_default()
            );
            Ok(())
        }
        other => {
            eprintln!("unknown verb {other:?}\n{USAGE}");
            return std::process::ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tamias-driver-sepia-config: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
