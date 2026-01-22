use colored::Colorize;
use gag::Gag;
use rodio::{Decoder, OutputStreamBuilder, Sink};
use std::{env, fs::File, io::BufReader};

pub fn play_audio(path: String) {
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

pub fn get_audio_runtime_path() -> String {
    if let Ok(sudo_uid) = env::var("SUDO_UID") {
        return format!("/run/user/{}", sudo_uid);
    }

    if let Ok(path) = env::var("XDG_RUNTIME_DIR") {
        return path;
    }

    String::from("/run/user/1000")
}
