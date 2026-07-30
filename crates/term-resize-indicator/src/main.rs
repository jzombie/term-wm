use clap::Parser;
use term_resize_indicator::run;

#[derive(Parser)]
#[command(version, about)]
struct Cli;

fn main() {
    Cli::parse();
    run().expect("resize indicator failed");
}
