use clap::{Parser, Subcommand};
use colored::Colorize;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
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
const REGEX: &str = "# BEGIN FOCUS BLOCK([\\s\\S]*?)# END FOCUS BLOCK";

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    Add { urls: Vec<String> },
    Remove { urls: Vec<String> },
    Start,
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
    running: &Arc<AtomicBool>,
    config: &Arc<Config>,
    is_background: bool,
    pid_path: &String,
) {
    running.store(false, Ordering::SeqCst);

    println!("{}", "\n[>] Cleaning up...".bold().cyan());
    let old_content =
        fs::read_to_string(&config.hosts_path).expect("[!] Failed to read host file content");
    let new_content = Regex::new(REGEX)
        .unwrap()
        .replace_all(&old_content, "")
        .to_string();
    let _ = fs::write(&config.hosts_path, &new_content);
    println!("{}", "[>] Exiting".bold().cyan());

    if !is_background {
        super::audio::play_audio(format!("{}/{}", config.data_directory, config.end_audio));
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

pub fn start_checker_thead(config: Arc<Config>, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            let blocked_content = build_blocked_content(&config);
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
    let regex = Regex::new(REGEX).unwrap();

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

    let content = fs::read_to_string(&config.hosts_path).expect("[!] Failed to read host file");
    if regex.is_match(&content) {
        println!("{}", "[+] Sites are blocked".bold().green());
    } else {
        println!("{}", "[+] Sites are not blocked".bold().green());
    }
}

pub fn stop_daemon(config: &Config) {
    let pid_path = format!("{}/focus.pid", config.log_directory);

    if let Ok(pid_str) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            println!("{}", format!("[>] Stopping daemon...").bold().cyan());

            let _ = Command::new("kill").arg(pid.to_string()).status();

            println!("{}", "[>] Cleaning up...".bold().cyan());

            let old_content = fs::read_to_string(&config.hosts_path)
                .expect("[!] Failed to read host file content");
            let new_content = Regex::new(REGEX)
                .unwrap()
                .replace_all(&old_content, "")
                .to_string();
            let _ = fs::write(&config.hosts_path, &new_content);

            thread::sleep(Duration::from_millis(500));
            let _ = fs::remove_file(pid_path);
        }
    } else {
        eprintln!(
            "{}",
            "[!] No active focus session found to stop"
                .bold()
                .red()
        );
    }

    let hosts_content =
        fs::read_to_string(&config.hosts_path).expect("[!] Failed to read host file");
    let regex = Regex::new(REGEX).unwrap();

    if regex.is_match(&hosts_content) {
        println!("{}", "[>] Sites are blocked. Unblocking...".bold().cyan());
        let new_content = Regex::new(REGEX)
            .unwrap()
            .replace_all(&hosts_content, "")
            .to_string();
        let _ = fs::write(&config.hosts_path, &new_content);
    } else {
        println!("{}", "[+] Sites are not blocked".bold().green());
    }
}

pub fn add_urls(urls: &Vec<String>, config: Config) {
    if urls.is_empty() {
        println!(
            "{}",
            "[!] Please provide a list of one or more URLs".bold().red()
        );
        return;
    }

    let mut config = config.clone();
    let mut urls = urls.clone();
    config.blocked_sites.append(&mut urls);
    save_config(&config).expect("[!] Failed to save configuration");
}

pub fn remove_urls(urls: &Vec<String>, config: Config) {
    if urls.is_empty() {
        println!(
            "{}",
            "[!] Please provide a list of one or more URLs".bold().red()
        );
        return;
    }

    let mut config = config.clone();
    let urls = urls.clone();
    config.blocked_sites.retain(|url| !urls.contains(url));
    save_config(&config).expect("[!] Failed to save configuration");
}

pub fn block_sites(config: &Config, forever: bool) {
    let blocked_content = build_blocked_content(config);
    let mut hosts_file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&config.hosts_path)
        .expect(&format!(
            "[!] Failed to open {}. Are you running as sudo?",
            &config.hosts_path
        ));
    let mut current_content = String::new();
    hosts_file
        .read_to_string(&mut current_content)
        .expect("[!] Failed to read host file content");

    let regex = Regex::new(REGEX).unwrap();
    if regex.is_match(&current_content) {
        println!(
            "{}",
            format!("[!] Blocking is already active").bold().yellow()
        );
        return;
    }

    if forever {
        println!(
            "{}",
            format!("[>] Blocking sites until you unblock them")
                .bold()
                .cyan()
        );
    } else {
        println!(
            "{}",
            format!("[>] Blocking sites for {} minutes", config.duration)
                .bold()
                .cyan()
        );
    }

    if let Err(e) = hosts_file.write(blocked_content.as_bytes()) {
        eprintln!(
            "{}",
            format!("[!] Failed to write to hosts file: {}", e)
                .bold()
                .red()
        );
        process::exit(1);
    }
}

fn build_blocked_content(config: &Config) -> String {
    let mut content = String::from("\n# BEGIN FOCUS BLOCK\n");
    for site in &config.blocked_sites {
        content.push_str(&format!("{}\t{}\n", &config.block_ip, site));
    }
    content.push_str("# END FOCUS BLOCK");
    content
}
