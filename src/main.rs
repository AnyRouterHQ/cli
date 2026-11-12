fn main() {
    // The only work before argument parsing: one panic::set_hook.
    #[cfg(feature = "native")]
    anyr_cli::crash::install();
    // Rust ignores SIGPIPE by default, so printing to a closed pipe
    // (`anyr ... | head`) panics. Restore the default so we exit quietly.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let code = anyr_cli::run(
        std::env::args().skip(1).collect(),
        std::env::vars().collect(),
    );
    std::process::exit(code);
}
