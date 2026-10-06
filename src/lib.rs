pub mod cli;
pub mod config;
pub mod exclude;
pub mod format;
pub mod model;
pub mod os;
pub mod scan;

pub fn run(args: Vec<std::ffi::OsString>) -> std::io::Result<()> {
    use std::io::{BufReader, BufWriter, Read, Write};
    let config = cli::parse(&args, true)?;
    if config.help {
        print!("{}", cli::HELP);
        return Ok(());
    }
    if config.version {
        println!("rcdu {} (ncdu 2.9 compatible)", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let model = if let Some(path) = &config.import {
        let input: Box<dyn Read> = if path == std::path::Path::new("-") {
            Box::new(std::io::stdin())
        } else {
            Box::new(os::input(path)?)
        };
        let mut input = BufReader::new(input);
        let mut prefix = [0; 4];
        input.read_exact(&mut prefix)?;
        let input = std::io::Cursor::new(prefix).chain(input);
        if prefix == [0x28, 0xb5, 0x2f, 0xfd] {
            format::json::read(BufReader::new(zstd::stream::read::Decoder::new(input)?))?
        } else {
            format::json::read(BufReader::new(input))?
        }
    } else {
        scan::scan(
            config.root.as_deref().unwrap_or(std::path::Path::new(".")),
            &config.scan,
        )?
    };
    if let Some((path, binary)) = &config.export {
        if *binary {
            return Err(os::invalid("binary output under implementation"));
        }
        let output: Box<dyn Write> = if path == std::path::Path::new("-") {
            Box::new(std::io::stdout())
        } else {
            Box::new(os::output(path)?)
        };
        let mut output = BufWriter::with_capacity(65536, output);
        if config.compress {
            let mut encoder =
                zstd::stream::write::Encoder::new(&mut output, config.compression_level)?;
            format::json::write(&model, &mut encoder, config.scan.extended)?;
            encoder.finish()?;
            output.flush()?;
        } else {
            format::json::write(&model, output, config.scan.extended)?;
        }
    }
    Ok(())
}


