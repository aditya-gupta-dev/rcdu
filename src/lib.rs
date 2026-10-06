pub mod cli;
pub mod config;
pub mod exclude;
pub mod model;
pub mod os;
pub mod scan;

pub fn run(args: Vec<std::ffi::OsString>) -> std::io::Result<()> {
    let config = cli::parse(&args, true)?;
    if config.help { print!("{}", cli::HELP); return Ok(()); }
    if config.version { println!("rcdu {}", env!("CARGO_PKG_VERSION")); return Ok(()); }
    if config.import.is_some() || config.export.is_some() { return Err(os::invalid("serialization is the next implementation milestone")); }
    scan::scan(config.root.as_deref().unwrap_or(std::path::Path::new(".")), &config.scan)?;
    Ok(())
}
