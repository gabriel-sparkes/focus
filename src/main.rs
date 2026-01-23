use clap::Parser;
use colored::Colorize;
use daemonize::Daemonize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path,
    process::{self, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

mod audio;
mod util;

fn main() {
    let args = util::Args::parse();

    let mut config = match util::load_config() {
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

    match &args.command {
        Some(util::Commands::Add {urls}) => {
            util::add_urls(urls, config);
            return;
        }
        Some(util::Commands::Status) => {
            util::check_status();
            return;
        }
        Some(util::Commands::Stop) => {
            util::stop_daemon();
            return;
        }
        None => {}
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
            .pid_file(&pid_path)
            .chroot("/")
            .working_directory(&config.log_directory)
            .stdout(stdout)
            .stderr(stderr);

        daemonize
            .start()
            .expect(&format!("{}", "[!] Error: daemonize failed"));
    } else {
        audio::play_audio(format!("{}/{}", config.data_directory, config.start_audio));
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
        util::ctrlc_handler(
            &handler_running,
            &handler_config,
            &handler_content,
            args.background,
            &pid_path,
        );
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
    util::start_checker_thead(thread_config, new_content, thread_running);
    thread::sleep(Duration::from_mins(config.duration));

    running.store(false, Ordering::SeqCst);
    thread::sleep(Duration::from_millis(100));

    println!("{}", "[>] Time's up! Unblocking sites".bold().cyan());
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
        audio::play_audio(format!("{}/{}", config.data_directory, config.end_audio));
    }
}
