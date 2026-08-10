fn main() {
    match linkvault_lib::crop_baseline::run() {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("newspaper clipping crop baseline failed: {error}");
            std::process::exit(1);
        }
    }
}
