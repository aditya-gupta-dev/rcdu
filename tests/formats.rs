use rcdu::{
    cli,
    config::{Config, GraphStyle, SortField},
    format::{binary, json},
    model::Kind,
};
use std::io::{BufReader, Cursor};
fn fixture() -> Vec<u8> {
    b"[1,99,{\"extension\":[null,true,false,-12.5e+3]},[{\"name\":\"/root\",\"asize\":4096,\"dsize\":4096,\"dev\":7},[{\"name\":\"a\",\"asize\":4096},{\"name\":\"bad\xff\\n\",\"asize\":5,\"dsize\":512,\"ino\":9,\"nlink\":3,\"hlnkc\":true,\"mtime\":12.75,\"uid\":0}], [{\"name\":\"b\",\"asize\":4096},{\"name\":\"link\",\"ino\":9,\"nlink\":3,\"asize\":5,\"dsize\":512,\"hlnkc\":true}]],null]".to_vec()
}
#[test]
fn json_byte_names_numbers_and_hardlinks_roundtrip() {
    let model = json::read(BufReader::with_capacity(1, Cursor::new(fixture()))).unwrap();
    assert_eq!(model.totals(model.root).shared_apparent, 5);
    let mut bytes = Vec::new();
    json::write(&model, &mut bytes, true).unwrap();
    let copied = json::read(Cursor::new(&bytes)).unwrap();
    assert_eq!(copied.totals(copied.root), model.totals(model.root));
    assert!(bytes.windows(4).any(|bytes| bytes == b"bad\xff"));
    let surrogate =
        b"[1,2,{},[{\"name\":\"/\"},{\"name\":\"\\ud83d\\ude00\",\"asize\":18446744073709551615}]]";
    let model = json::read(Cursor::new(surrogate)).unwrap();
    let child = model.children(model.root).next().unwrap();
    assert_eq!(model.name(child), "😀".as_bytes());
    assert_eq!(model.entry(child).apparent, u64::MAX);
}
#[test]
fn malformed_imports_never_create_unsafe_children() {
    for name in ["..", ".", "a/b", "a\\u0000b", ""] {
        let bytes = format!("[1,2,{{}},[{{\"name\":\"/\"}},{{\"name\":\"{name}\"}}]]");
        assert!(json::read(Cursor::new(bytes)).is_err());
    }
    for bytes in [
        b"[1,2,{},[{\"name\":\"/\",\"asize\":18446744073709551616}]]".as_slice(),
        b"[1,2,{},[{\"name\":\"/\"},{\"name\":\"\\ud800\"}]]",
        b"[1,2,{},[{\"name\":\"/\"},]]",
        b"[1,2,{},[{\"name\":\"/\"}]]garbage",
    ] {
        assert!(json::read(Cursor::new(bytes)).is_err());
    }
    let bytes = fixture();
    for length in 0..bytes.len() {
        assert!(json::read(Cursor::new(&bytes[..length])).is_err());
    }
}
#[test]
fn binary_own_cumulative_fields_and_cache_eviction() {
    let mut input = b"[1,2,{},[{\"name\":\"/root\",\"asize\":100,\"dsize\":512}".to_vec();
    for number in 0..3000 {
        input.extend_from_slice(
            format!(",{{\"name\":\"file-{number:08}\",\"asize\":42,\"dsize\":512}}").as_bytes(),
        );
    }
    input.extend_from_slice(b"]]");
    let model = json::read(Cursor::new(input)).unwrap();
    let mut bytes = Vec::new();
    binary::write(&model, &mut bytes, 4096, 1, true).unwrap();
    let mut reader = binary::Reader::open(Cursor::new(&bytes)).unwrap();
    let root = reader.get(reader.root).unwrap();
    assert_eq!(root.stat.apparent, 100);
    assert_eq!(root.totals.apparent, 100 + 3000 * 42);
    assert_eq!(reader.children(&root).unwrap().len(), 3000);
    assert_eq!(reader.cached_blocks(), 8);
    let copied = reader.import().unwrap();
    assert_eq!(copied.totals(copied.root), model.totals(model.root));
    for position in [0, 8, 12, bytes.len() - 4] {
        let mut bad = bytes.clone();
        bad[position] ^= 0x80;
        let corrupt = binary::Reader::open(Cursor::new(bad)).and_then(|mut reader| reader.import());
        assert!(corrupt.is_err());
    }
    assert!(binary::Reader::open(Cursor::new(&bytes[..bytes.len() - 1])).is_err());
}
#[test]
fn binary_preserves_links_extended_and_raw_bytes() {
    let model = json::read(Cursor::new(fixture())).unwrap();
    let mut bytes = Vec::new();
    binary::write(&model, &mut bytes, 4096, 1, true).unwrap();
    let mut reader = binary::Reader::open(Cursor::new(bytes)).unwrap();
    let imported = reader.import().unwrap();
    assert_eq!(model.totals(model.root), imported.totals(imported.root));
    let mut ids = vec![imported.root];
    let mut found = false;
    while let Some(id) = ids.pop() {
        ids.extend(imported.children(id));
        if imported.entry(id).kind() == Kind::Hardlink && imported.name(id).starts_with(b"bad") {
            assert_eq!(imported.extended(id).unwrap().mtime, 12);
            found = true;
        }
    }
    assert!(found);
}
#[test]
fn cli_clusters_separator_precedence_and_optional_config() {
    let args = ["-rr", "-t4", "-o-", "--sort=mtime-desc", "--", "-directory"].map(Into::into);
    let config = cli::parse(&args, false).unwrap();
    assert_eq!(config.can_delete, Some(false));
    assert_eq!(config.can_shell, Some(false));
    assert_eq!(config.scan.workers, 4);
    assert_eq!(config.sort, SortField::Mtime);
    assert!(config.descending);
    let mut config = Config::default();
    config.config_bytes(b"\n# comment\n@--future-option=anything\n--threads=2\n--graph-style eigth-block\n--hide-hidden\n--compress-level 20").unwrap();
    assert_eq!(config.scan.workers, 2);
    assert_eq!(config.graph_style, GraphStyle::Eighth);
    assert!(!config.hidden);
    assert_eq!(config.compression_level, 20);
    assert!(config.config_bytes(b"--threads 256").is_err());
    assert!(cli::parse(&["-o".into()], false).is_err());
    assert!(cli::parse(&["--extended=yes".into()], false).is_err());
}
#[test]
fn errors_and_exclusions_are_independent_of_field_order() {
    for fields in [
        "\"excluded\":\"pattern\",\"nlink\":2",
        "\"nlink\":2,\"excluded\":\"pattern\"",
    ] {
        let input = format!("[1,2,{{}},[{{\"name\":\"/\"}},{{\"name\":\"file\",{fields}}}]]");
        let model = json::read(Cursor::new(input)).unwrap();
        let child = model.children(model.root).next().unwrap();
        assert_eq!(model.entry(child).kind(), Kind::Pattern);
    }
}

#[test]
fn unknown_large_numbers_and_unknown_link_counts_remain_compatible() {
    let input=b"[1,12,{\"future\":123456789012345678901234567890e+400},[{\"name\":\"/root\"},[{\"name\":\"a\"},{\"name\":\"x\",\"ino\":1,\"hlnkc\":true,\"asize\":7}], [{\"name\":\"b\"},{\"name\":\"y\",\"ino\":1,\"hlnkc\":true,\"asize\":7}]]]";
    let model = json::read(Cursor::new(input)).unwrap();
    assert_eq!(model.totals(model.root).shared_apparent, 0);
    for child in model.children(model.root) {
        assert_eq!(model.totals(child).shared_apparent, 7);
    }
    let mut bytes = Vec::new();
    binary::write(&model, &mut bytes, 4096, 1, true).unwrap();
    let mut reader = binary::Reader::open(Cursor::new(bytes)).unwrap();
    let root = reader.get(reader.root).unwrap();
    for child in reader.children(&root).unwrap() {
        assert_eq!(child.totals.shared_apparent, 7);
    }
    assert_eq!(
        reader.import().unwrap().totals(model.root),
        model.totals(model.root)
    );
}

#[test]
fn compact_parent_runs_preserve_nested_paths_and_capacity_errors() {
    use rcdu::model::{Entry, EntryId, NONE, Part};
    use rcdu::os::Observation;
    assert_eq!(std::mem::size_of::<Entry>(), 24);
    let mut part = Part::default();
    let root = part
        .add(
            0,
            b"/root",
            NONE,
            Observation {
                kind: Kind::Directory,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let a = part
        .add(
            0,
            b"a",
            root,
            Observation {
                kind: Kind::Directory,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    for (name, parent) in [
        (b"first".as_slice(), a),
        (b"second", a),
        (b"third", root),
        (b"fourth", a),
    ] {
        part.add(0, name, parent, Observation::default(), false)
            .unwrap();
    }
    let model = rcdu::model::Model {
        parts: vec![part],
        root,
    };
    for (slot, path) in [
        (2, "/root/a/first"),
        (3, "/root/a/second"),
        (4, "/root/third"),
        (5, "/root/a/fourth"),
    ] {
        assert_eq!(model.path(EntryId(slot)), std::path::PathBuf::from(path));
    }
    assert_eq!(model.parts[0].parents.len(), 5);
    let mut part = Part::default();
    assert!(
        part.add(255, b"/", NONE, Observation::default(), false)
            .is_err()
    );
    let maximum=json::read(Cursor::new(b"[1,2,{},[{\"name\":\"/\",\"dsize\":18446744073709551615},{\"name\":\"child\",\"dsize\":18446744073709551615,\"asize\":18446744073709551615}]]")).unwrap();
    assert_eq!(maximum.totals(maximum.root).allocated, u64::MAX);
    assert_eq!(maximum.totals(maximum.root).apparent, u64::MAX);
    for args in [
        ["--export-block-size", "3"],
        ["--export-block-size", "16001"],
        ["--compress-level", "0"],
        ["--threads", "256"],
    ] {
        assert!(cli::parse(&args.map(Into::into), false).is_err());
    }
}
