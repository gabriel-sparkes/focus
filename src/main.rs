use clap::Parser;
use colored::Colorize;
use daemonize::Daemonize;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    process::{self, Command},
    sync::Arc,
    thread,
    time::Duration,
};

const CONFIG_PATH: &str = "config.toml";
const CHECK_INTERVAL: u64 = 5;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    duration: Option<u64>,

    #[arg(short, long, default_value_t = false)]
    background: bool,

    #[arg(short, long)]
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    hosts_path: String,
    block_ip: String,
    blocked_sites: Vec<String>,
    duration: u64,
    working_directory: String,
}

fn main() {
    let args = Args::parse();
    let run_in_background = args.background;

    let mut config = match load_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "{}",
                format!("[!] Error parsing config.toml: {}", e).bold().red()
            );
            std::process::exit(1);
        }
    };

    if let Some(path) = args.path {
        config.hosts_path = path;
    }

    if let Some(duration) = args.duration {
        config.duration = duration;
    }

    let config = Arc::new(config);

    if run_in_background {
        let stdout = File::create(format!("{}/focus.out", config.working_directory)).unwrap();
        let stderr = File::create(format!("{}/focus.err", config.working_directory)).unwrap();

        let daemonize = Daemonize::new()
            .pid_file(format!("{}/focus.pid", config.working_directory))
            .chroot("/")
            .working_directory(&config.working_directory)
            .stdout(stdout)
            .stderr(stderr);

        daemonize
            .start()
            .expect(&format!("{}", "Error, daemonize failed".bold().red()));
    }

    let original_content = match fs::read_to_string(&config.hosts_path) {
        Ok(content) => Arc::new(content),
        Err(e) => {
            eprintln!(
                "{}",
                format!(
                    "[!] Failed to read hosts file. Are you running as sudo? Error: {}",
                    e
                )
                .bold()
                .red()
            );
            process::exit(1);
        }
    };
    let handler_config = Arc::clone(&config);
    let handler_content = Arc::clone(&original_content);

    ctrlc::set_handler(move || {
        save_config(&handler_config).unwrap();
        println!("{}", "\n[>] Cleaning up...".bold().cyan());
        let _ = fs::write(&handler_config.hosts_path, &*handler_content);
        println!("{}", "[>] Exiting".bold().cyan());
        process::exit(0);
    })
    .expect("Error setting Ctrl-C handler");

    let mut new_content = String::from("\n# BEGIN FOCUS BLOCK\n");
    for site in &config.blocked_sites {
        new_content.push_str(&format!("{}\t{}\n", &config.block_ip, site));
    }
    new_content.push_str("# END FOCUS BLOCK");

    let mut hosts_file = OpenOptions::new()
        .append(true)
        .open(&config.hosts_path)
        .expect(
            &format!("[!] Failed to open {}", &config.hosts_path)
                .bold()
                .red(),
        );

    println!(
        "{}",
        format!("[>] Blocking sites for {} minutes", config.duration)
            .bold()
            .cyan()
    );
    if let Err(e) = hosts_file.write(&*new_content.as_bytes()) {
        eprintln!(
            "{}",
            format!("[!] Failed to write to hosts file: {}", e)
                .bold()
                .red()
        );
        process::exit(1);
    }

    println!("{}", "[>] Flushing DNS cache".bold().cyan());
    Command::new("resolvectl")
        .arg("flush-caches")
        .output()
        .expect(&format!("{}", "[!] Failed to flush DNS cache".bold().red()));
    let thread_config = Arc::clone(&config);
    start_checker_thead(thread_config, new_content);
    thread::sleep(Duration::from_mins(config.duration));

    println!("{}", "Time's up! Unblocking sites.".bold().cyan());
    if let Err(e) = fs::write(&config.hosts_path, &*original_content) {
        eprintln!(
            "{}",
            format!(
                "CRITICAL: Failed to restore hosts file. Please fix manually at {}",
                &config.hosts_path
            )
            .bold()
            .red()
        );
        eprintln!("{}", format!("Error: {}", e).bold().red());
    }
}

fn load_config() -> Result<Config, toml::de::Error> {
    let content = fs::read_to_string(CONFIG_PATH)
        .expect(&format!("[!] Could not read {}", CONFIG_PATH).bold().red());

    let config = toml::from_str(&content);
    config
}

fn save_config(config: &Config) -> Result<(), io::Error> {
    let toml_string = toml::to_string(config).expect(&format!(
        "{}",
        "[!] Could not encode config to TOML".bold().red()
    ));
    fs::write(CONFIG_PATH, toml_string)
}

fn start_checker_thead(config: Arc<Config>, blocked_content: String) {
    thread::spawn(move || {
        loop {
            if let Ok(current_content) = fs::read_to_string(&config.hosts_path) {
                if !current_content.contains(&blocked_content) {
                    let mut hosts_file = OpenOptions::new()
                        .append(true)
                        .open(&config.hosts_path)
                        .expect(&format!("Failed to open {}", &config.hosts_path));
                    println!(
                        "{}",
                        "[!] Tamper detected! Reblocking sites...".bold().red()
                    );

                    hosts_file
                        .write(blocked_content.as_bytes())
                        .expect(&format!("{}", "[!] Write to file failed".bold().red()));
                }
            }

            thread::sleep(Duration::from_secs(CHECK_INTERVAL));
        }
    });
}
