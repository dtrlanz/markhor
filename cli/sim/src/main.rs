mod console;
mod sim;

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use markhor_core::chat::ChatUi;

#[tokio::main]
async fn main() {
    let seed = parse_seed().unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });

    let ui: Arc<dyn ChatUi> = Arc::new(console::ConsoleUi::default());
    sim::Simulation::new(ui, seed).run().await;
}

/// Reads an optional `--seed <n>` argument.
fn parse_seed() -> Option<u64> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--seed" {
            return args.next().and_then(|v| v.parse().ok());
        }
    }
    None
}
