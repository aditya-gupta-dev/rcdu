//! Deterministic byte-name/metadata/totals oracle, independent of worker and wire order.
use rcdu::{format::json, model::Kind};
use std::io::{self, BufReader};
fn main() -> io::Result<()> {
    let model = json::read(BufReader::new(io::stdin()))?;
    let mut lines = Vec::new();
    for (worker, part) in model.parts.iter().enumerate() {
        for slot in 0..part.entries.len() {
            let id = rcdu::model::EntryId(((worker as u32) << 24) | slot as u32);
            let name = if slot == 0 && worker == 0 {
                Vec::new()
            } else {
                rcdu::os::bytes(&model.path(id)).to_vec()
            };
            let hex: String = name.iter().map(|byte| format!("{byte:02x}")).collect();
            let own = model.entry(id);
            let totals = model.totals(id);
            let stat = model.metadata(id);
            let dir = model
                .directory_id(id)
                .map(|id| &model.directories[id as usize]);
            let identity = (own.kind() == Kind::Hardlink).then_some((stat.device, stat.inode));
            lines.push(format!(
                "{hex}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{:?}",
                own.kind().wire(),
                own.allocated(),
                own.apparent,
                totals.allocated,
                totals.apparent,
                totals.shared_allocated,
                totals.shared_apparent,
                totals.items,
                stat.links,
                dir.is_some_and(|dir| dir.read_error),
                dir.is_some_and(|dir| dir.descendant_error),
                identity,
                model.extended(id).map(|stat| (
                    stat.present,
                    stat.mtime,
                    stat.uid,
                    stat.gid,
                    stat.mode
                ))
            ));
        }
    }
    lines.sort_unstable();
    for line in lines {
        println!("{line}");
    }
    Ok(())
}
