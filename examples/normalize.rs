//! Byte-aware interoperability oracle; record ordering and compression do not affect the digest.
use rcdu::{format::json, model::NONE};
use std::io::{self, BufReader};
fn main() -> io::Result<()> {
    let model = json::read(BufReader::new(std::io::stdin()))?;
    let mut records = Vec::new();
    let mut pending = vec![model.root];
    while let Some(id) = pending.pop() {
        let mut names = Vec::new();
        let mut current = id;
        while current != model.root && current != NONE {
            names.push(model.name(current));
            current = model.entry(current).parent;
        }
        let mut path = Vec::new();
        for component in names.into_iter().rev() {
            path.extend_from_slice(component);
            path.push(b'/');
        }
        let name: String = path.iter().map(|byte| format!("{byte:02x}")).collect();
        let entry = model.entry(id);
        let totals = model.totals(id);
        let stat = model.observation(id);
        records.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}",
            name,
            entry.kind().wire(),
            entry.allocated(),
            entry.apparent,
            totals.allocated,
            totals.apparent,
            totals.shared_allocated,
            totals.shared_apparent,
            totals.items,
            stat.links,
            model
                .directory(id)
                .is_some_and(|dir| dir.read_error || dir.descendant_error),
            model.extended(id)
        ));
        pending.extend(model.children(id));
    }
    records.sort_unstable();
    for record in records {
        println!("{record}");
    }
    Ok(())
}
