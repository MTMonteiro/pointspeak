//! Cola um texto no app em foco pelo injector do app, para teste manual (precisa de tela).
//!
//! `cargo run -p pointspeak --example inject -- "texto"`
//!
//! Espera 3 s antes de colar, para dar tempo de focar o app de destino (editor, terminal). O
//! clipboard deve voltar ao conteúdo anterior logo depois da colagem.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use pointspeak_lib::hud::current_display_server;
use pointspeak_lib::injector;

fn main() -> ExitCode {
    let text: Vec<String> = std::env::args().skip(1).collect();
    if text.is_empty() {
        eprintln!("uso: inject <texto>");
        return ExitCode::from(2);
    }
    let text = text.join(" ");
    let display = current_display_server();
    eprintln!("servidor gráfico: {display:?}; colando em 3 s...");
    thread::sleep(Duration::from_secs(3));
    match injector::inject(&text, display) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e} (HUD: \"{}\")", e.kind().message());
            ExitCode::FAILURE
        }
    }
}
