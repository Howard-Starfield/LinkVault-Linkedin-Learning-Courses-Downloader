fn main() {
    match linkvault_lib::durability_baseline::run() {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("newspaper clipping note durability baseline failed: {error}");
            std::process::exit(1);
        }
    }
}
