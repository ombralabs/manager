// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  // `ombra flow …` — командная строка автоматизации (спецификация А2): своё ядро без окна, Tauri не поднимается.
  let mut args = std::env::args().skip(1);
  let command = args.next();
  if command.as_deref() == Some("--vault-guard") {
    if let (Some(parent), Some(root)) = (args.next().and_then(|value| value.parse::<u32>().ok()), args.next()) {
      let _ = ombra_lib::security::guard(parent, ombra_lib::paths::Paths::new(root));
    }
    return;
  }
  if command.as_deref() == Some("flow") {
    std::process::exit(ombra_lib::cli::main(args.collect()));
  }
  ombra_lib::run();
}
