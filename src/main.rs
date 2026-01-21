use clap::Parser;
use colored::Colorize;
use daemonize::Daemonize;
use gag::Gag;
use rodio::{Decoder, OutputStreamBuilder, Sink};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Write},
    path,
    process::{self, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const CONFIG_PATH: &str = "/usr/local/etc/focus/config.toml";
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

    #[arg(short, long, num_args=1..)]
    add: Option<Vec<String>>,

    #[arg(long)]
    config: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    hosts_path: String,
    block_ip: String,
    blocked_sites: Vec<String>,
    duration: u64,
    data_directory: String,
    log_directory: String,
    start_audio: String,
    end_audio: String,
}

fn main() {
    let args = Args::parse();

    let mut config = match load_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "{}",
                format!("[!] Error parsing config.toml: {}", e).bold().red()
            );
            process::exit(1);
        }
    };

    if let Some(path) = args.path {
        config.hosts_path = path;
    }

    if let Some(duration) = args.duration {
        config.duration = duration;
    }

    if let Some(mut new_sites) = args.add {
        config.blocked_sites.append(&mut new_sites);
    }

    let config = Arc::new(config);

    let pid_path = format!("{}/focus.pid", config.log_directory);
    let out_path = format!("{}/focus.out", config.log_directory);
    let err_path = format!("{}/focus.err", config.log_directory);

    if path::Path::new(&pid_path).exists() {
        println!(
            "{}",
            "[!] Warning: Stale PID file found. Deleting..."
                .bold()
                .yellow()
        );
        let _ = fs::remove_file(&pid_path);
    }

    if args.background {
        println!("{}", "[>] Moving to background...".bold().cyan());

        let stdout = File::create(out_path).unwrap();
        let stderr = File::create(err_path).unwrap();

        let daemonize = Daemonize::new()
            .pid_file(pid_path)
            .chroot("/")
            .working_directory(&config.log_directory)
            .stdout(stdout)
            .stderr(stderr);

        daemonize
            .start()
            .expect(&format!("{}", "[!] Error: daemonize failed"));
    } else {
        play_audio(format!("{}/{}", config.data_directory, config.start_audio));
    }

    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);

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

    let handler_running = Arc::clone(&running);
    let handler_config = Arc::clone(&config);
    let handler_content = Arc::clone(&original_content);

    ctrlc::set_handler(move || {
        handler_running.store(false, Ordering::SeqCst);
        save_config(&handler_config).unwrap();

        println!("{}", "\n[>] Cleaning up...".bold().cyan());
        let _ = fs::write(&handler_config.hosts_path, &*handler_content);
        println!("{}", "[>] Exiting".bold().cyan());

        if !args.background {
            play_audio(format!(
                "{}/{}",
                handler_config.data_directory, handler_config.end_audio
            ));
        }
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
        .expect(&format!(
            "[!] Failed to open {}. Are you running as sudo?",
            &config.hosts_path
        ));

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
        .expect(&format!("{}", "[!] Failed to flush DNS cache"));

    let thread_config = Arc::clone(&config);
    start_checker_thead(thread_config, new_content, thread_running);
    thread::sleep(Duration::from_mins(config.duration));

    running.store(false, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(100));

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
    if !args.background {
        play_audio(format!("{}/{}", config.data_directory, config.end_audio));
    }
}

fn load_config() -> Result<Config, toml::de::Error> {
    let content =
        fs::read_to_string(CONFIG_PATH).expect(&format!("[!] Could not read {}", CONFIG_PATH));

    let config = toml::from_str(&content);
    config
}

fn save_config(config: &Config) -> Result<(), io::Error> {
    let toml_string =
        toml::to_string(config).expect(&format!("{}", "[!] Could not encode config to TOML"));
    fs::write(CONFIG_PATH, toml_string)
}

fn start_checker_thead(config: Arc<Config>, blocked_content: String, running: Arc<AtomicBool>) {
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

fn play_audio(path: String) {
    let _print_gag = Gag::stderr().unwrap();

    let audio_runtime_path = get_audio_runtime_path();
    if env::var("XDG_RUNTIME_DIR").is_err() {
        unsafe {
            env::set_var("XDG_RUNTIME_DIR", audio_runtime_path);
        }
    }

    if let Ok(stream) = OutputStreamBuilder::open_default_stream() {
        let sink = Sink::connect_new(stream.mixer());
        if let Ok(file) = File::open(&path) {
            let reader = BufReader::new(file);
            if let Ok(source) = Decoder::new(reader) {
                sink.append(source);
                sink.sleep_until_end();
            }
        }
    } else {
        eprintln!(
            "{}",
            "[!] Audio device unavailable (Host is down)"
                .bold()
                .yellow()
        );
    }
}

fn get_audio_runtime_path() -> String {
    if let Ok(sudo_uid) = env::var("SUDO_UID") {
        return format!("/run/user/{}", sudo_uid);
    }

    if let Ok(path) = env::var("XDG_RUNTIME_DIR") {
        return path;
    }

    String::from("/run/user/1000")
}