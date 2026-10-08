use rcdu::{
    cli::{self, Config, Graph, Sort},
    model::Kind,
    scan,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rcdu-config-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn clustered_flags_defaults_config_suppression_and_bounds() {
    assert_eq!(Config::default().scan.threads, 0);
    let args = ["-rr", "-t4", "--sort=mtime-asc", "-o-", "--", "-root"].map(Into::into);
    let config = cli::parse(&args, false).unwrap();
    assert_eq!(config.scan.threads, 4);
    assert_eq!(config.can_delete, Some(false));
    assert_eq!(config.can_shell, Some(false));
    assert!(config.sort == Sort::Mtime);
    assert!(!config.descending);
    assert_eq!(config.root.unwrap(), PathBuf::from("-root"));
    let mut config = Config::default();
    config.config_bytes(b"# comment\n\n--threads=2\n@--threads=999\n@--future-option=x\n--graph-style eigth-block").unwrap();
    assert_eq!(config.scan.threads, 2);
    assert!(config.graph_style == Graph::Eighth);
    for args in [
        ["-t", "256"],
        ["--export-block-size", "3"],
        ["--compress-level", "21"],
        ["--extended=yes", ""],
    ] {
        assert!(cli::parse(&args.map(Into::into), false).is_err());
    }
    assert!(config.config_bytes(&vec![b'x'; 4097]).is_err());
}
#[test]
fn directory_patterns_and_cache_markers_preserve_own_size_rules() {
    let fixture = Fixture::new();
    for name in ["skip", "cache", "keep"] {
        fs::create_dir(fixture.0.join(name)).unwrap();
        fs::write(fixture.0.join(name).join("data"), b"data").unwrap();
    }
    fs::write(
        fixture.0.join("cache/CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55\ncomment",
    )
    .unwrap();
    fs::write(fixture.0.join("keep/CACHEDIR.TAG"), b"not a signature").unwrap();
    let mut config = Config::default();
    config.scan.threads = 4;
    config.scan.exclude_caches = true;
    config.scan.exclusions.add(b"skip/").unwrap();
    let model = scan::scan(&fixture.0, &config.scan).unwrap();
    let entries: Vec<_> = model.children(0).collect();
    for name in [b"skip".as_slice(), b"cache"] {
        let entry = entries
            .iter()
            .find(|entry| model.name(**entry) == name)
            .unwrap();
        assert_eq!(model.entry(*entry).kind(), Kind::Excluded);
        assert_eq!(model.entry(*entry).allocated(), 0);
    }
    let kept = entries
        .iter()
        .find(|entry| model.name(**entry) == b"keep")
        .unwrap();
    assert_eq!(
        model.children(model.directory_id(*kept).unwrap()).count(),
        2
    );
    assert_eq!(model.directories[0].totals.items, 5);
}
