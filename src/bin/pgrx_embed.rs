use std::process;

fn main() {
    // This binary is used by pgrx for testing and development
    // It embeds the extension for easier testing
    eprintln!("This binary is managed by pgrx. Use `cargo pgrx test` instead.");
    process::exit(1);
}