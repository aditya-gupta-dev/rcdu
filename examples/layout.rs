//! Storage evidence; this diagnostic is not an accepted application timing benchmark.
use rcdu::{model, scan};
use std::{env, io, mem::size_of, path::Path};
fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    let options = scan::ScanOptions {
        workers: args
            .get(2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(1),
        ..Default::default()
    };
    let model = scan::scan(Path::new(args.get(1).map_or(".", String::as_str)), &options)?;
    let entries = model.len();
    let names: usize = model.parts.iter().map(|part| part.names.len()).sum();
    let parent_runs: usize = model.parts.iter().map(|part| part.parents.len()).sum();
    let mut length_bytes = 0;
    let mut capacity_bytes = 0;
    for part in &model.parts {
        macro_rules! add {
            ($field:ident, $type:ty) => {{
                length_bytes += part.$field.len() * size_of::<$type>();
                capacity_bytes += part.$field.capacity() * size_of::<$type>();
            }};
        }
        add!(entries, model::Entry);
        add!(parents, model::ParentRun);
        add!(names, u8);
        add!(directories, model::Directory);
        add!(hardlinks, model::Hardlink);
        add!(extended, (model::EntryId, rcdu::os::Extended));
    }
    println!(
        "{{\"entries\":{entries},\"entry_bytes\":{},\"name_bytes\":{names},\"parent_runs\":{parent_runs},\"length_bytes\":{length_bytes},\"capacity_bytes\":{capacity_bytes}}}",
        size_of::<model::Entry>()
    );
    Ok(())
}
