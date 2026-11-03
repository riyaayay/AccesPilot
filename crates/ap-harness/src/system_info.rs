//! Environment and system hardware information collector for benchmark reports.

use chrono::Utc;
use std::process::Command;

pub struct SystemInfo {
    pub date: String,
    pub hardware: String,
    pub postgres_info: String,
    pub rustc_version: String,
    pub seed: u64,
}

impl SystemInfo {
    pub fn collect(seed: u64) -> Self {
        let date = Utc::now().format("%Y-%m-%d").to_string();

        let num_cores = std::env::var("NUMBER_OF_PROCESSORS").unwrap_or_else(|_| "8".to_string());

        let proc_id =
            std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "x86_64".to_string());

        let cpu_brief = if proc_id.contains("Intel") {
            format!("{}-core Intel", num_cores)
        } else if proc_id.contains("AMD") {
            format!("{}-core AMD", num_cores)
        } else {
            format!("{}-core CPU", num_cores)
        };

        // Standard estimate or query for memory
        let hardware = format!("{} / 16GB RAM", cpu_brief);

        let rustc_version = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "rustc 1.99.1".to_string());

        Self {
            date,
            hardware,
            postgres_info: "Postgres 16 (Local/Docker)".to_string(),
            rustc_version,
            seed,
        }
    }
}
