use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    process::{self, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const CHECK_INTERVAL: u64 = 5;
const CONFIG_PATH: &str = "/usr/local/etc/focus/config.toml";

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    Add { urls: Vec<String> },
    Status,
    Stop,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]

pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(short, long)]
    pub duration: Option<u64>,

    #[arg(short, long, default_value_t = false)]
    pub background: bool,

    #[arg(short, long)]
    pub path: Option<String>,

    #[arg(long)]
    pub config: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub hosts_path: String,
    pub block_ip: String,
    pub blocked_sites: Vec<String>,
    pub duration: u64,
    pub data_directory: String,
    pub log_directory: String,
    pub start_audio: String,
    pub end_audio: String,
}

pub fn ctrlc_handler(
    handler_running: &Arc<AtomicBool>,
    handler_config: &Arc<Config>,
    handler_content: &Arc<String>,
    is_background: bool,
    pid_path: &String,
) {
    handler_running.store(false, Ordering::SeqCst);
    save_config(&handler_config).unwrap();

    println!("{}", "\n[>] Cleaning up...".bold().cyan());
    let _ = fs::write(&handler_config.hosts_path, &**handler_content);
    println!("{}", "[>] Exiting".bold().cyan());

    if !is_background {
        super::audio::play_audio(format!(
            "{}/{}",
            handler_config.data_directory, handler_config.end_audio
        ));
    }
    let _ = fs::remove_file(pid_path);
    process::exit(0);
}

pub fn load_config() -> Result<Config, toml::de::Error> {
    let content =
        fs::read_to_string(CONFIG_PATH).expect(&format!("[!] Could not read {}", CONFIG_PATH));

    let config = toml::from_str(&content);
    config
}

pub fn save_config(config: &Config) -> Result<(), io::Error> {
    let toml_string =
        toml::to_string(config).expect(&format!("{}", "[!] Could not encode config to TOML"));
    fs::write(CONFIG_PATH, toml_string)
}

pub fn start_checker_thead(config: Arc<Config>, blocked_content: String, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            if let Ok(current_content) = fs::read_to_string(&config.hosts_path) {
                if !current_content.contains(&blocked_content) {
                    let mut hosts_file = OpenOptions::new()
                        .append(true)
                        .open(&config.hosts_path)
                        .expect(&format!(
                            "Failed to open {}. Are you running as sudo?",
                            &config.hosts_path
                        ));
                    println!(
                        "{}",
                        "[!] Tamper detected! Reblocking sites...".bold().red()
                    );

                    hosts_file
                        .write(blocked_content.as_bytes())
                        .expect(&format!("{}", "[!] Write to file failed"));
                }
            }

            thread::sleep(Duration::from_secs(CHECK_INTERVAL));
        }
    });
}

pub fn check_status() {
    let config = load_config().unwrap_or_else(|_| {
        eprintln!(
            "{}",
            "[!] Could not load config to check status".bold().red()
        );
        process::exit(1);
    });
    let pid_path = format!("{}/focus.pid", config.log_directory);
    if Path::new(&pid_path).exists() {
        println!("{}", "[+] Focus is running".bold().green());
    } else {
        println!("{}", "[+] Focus is not running".bold().green());
    }
}

pub fn stop_daemon() {
    let config = load_config().expect("Could not load config");
    let pid_path = format!("{}/focus.pid", config.log_directory);

    if let Ok(pid_str) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            println!(
                "{}",
                format!("[>] Stopping daemon (PID: {})...", pid)
                    .bold()
                    .cyan()
            );

            let _ = Command::new("kill").arg(pid.to_string()).status();

            thread::sleep(Duration::from_millis(500));
            let _ = fs::remove_file(pid_path);
        }
    } else {
        eprintln!(
            "{}",
            "[!] No active focus session found to stop".bold().red()
        );
    }
}

pub fn add_urls(urls: &Vec<String>, config: Config) {
    let mut config = config.clone();
    let mut urls = urls.clone();
    config.blocked_sites.append(&mut urls);
    save_config(&config).unwrap();
}
