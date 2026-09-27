fn main() {
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a| a == "--reminder-setup" || a == "--reminder-remove") {
        let remove = std::env::args().any(|a| a == "--reminder-remove");
        if let Err(error) = markerup::reminders::setup_service(remove) {
            eprintln!("Reminder service setup: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "linux")]
    if std::env::args().any(|arg| arg == "--reminder-service") {
        if let Err(error) = markerup::reminders::run_service() {
            eprintln!("Reminder service: {error}");
            std::process::exit(1);
        }
        return;
    }
    markerup::run();
}
