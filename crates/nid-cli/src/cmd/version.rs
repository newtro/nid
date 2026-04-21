//! `nid version` — current + latest-known (placeholder) + channel.

use anyhow::Result;

pub fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let channel = std::env::var("NID_UPDATE_CHANNEL").unwrap_or_else(|_| "stable".into());
    println!("nid {current}");
    println!("channel: {channel}");
    Ok(())
}
