use rcdu::{format::json, model::Kind};
use std::io::{BufReader, Cursor};
fn fixture() -> Vec<u8> {
    b"[1,2,{},[{\"name\":\"/root\",\"dev\":1,\"asize\":4096,\"dsize\":4096},[{\"name\":\"a\",\"asize\":4096,\"dsize\":4096},{\"name\":\"h1\",\"asize\":6000,\"dsize\":8192,\"ino\":42,\"nlink\":4,\"hlnkc\":true},{\"name\":\"h2\",\"asize\":6000,\"dsize\":8192,\"ino\":42,\"nlink\":4,\"hlnkc\":true},{\"name\":\"plain\",\"asize\":1000,\"dsize\":4096}],[{\"name\":\"b\",\"asize\":4096,\"dsize\":4096},{\"name\":\"h3\",\"asize\":6000,\"dsize\":8192,\"ino\":42,\"nlink\":4,\"hlnkc\":true},{\"name\":\"sparse\",\"asize\":1073741824}]]]".to_vec()
}
#[test]
fn shared_hardlinks_roundtrip_and_exact_directory_sizes() {
    let model = json::read(BufReader::with_capacity(1, Cursor::new(fixture()))).unwrap();
    let totals = model.directories[0].totals;
    assert_eq!(totals.allocated, 24576);
    assert_eq!(totals.apparent, 1073761112);
    assert_eq!(totals.shared_allocated, 8192);
    assert_eq!(totals.items, 7);
    let mut data = Vec::new();
    json::write(&model, &mut data, true).unwrap();
    assert_eq!(
        json::read(Cursor::new(data)).unwrap().directories[0].totals,
        totals
    );
}
#[test]
fn legacy_bytes_surrogates_unknown_fields_fractional_mtime_and_saturation() {
    let data = b"[1,99,{\"future\":123456789012345678901234567890e+400},[{\"name\":\"/\",\"dsize\":18446744073709551615},{\"name\":\"raw\xff\",\"asize\":18446744073709551615,\"mtime\":12.345,\"uid\":0},{\"name\":\"\\ud83d\\ude00\",\"dsize\":18446744073709551615}],null]";
    let model = json::read(Cursor::new(data)).unwrap();
    assert_eq!(model.directories[0].totals.allocated, u64::MAX);
    let first = model.children(0).next().unwrap();
    assert_eq!(model.name(first), b"raw\xff");
    assert_eq!(model.extended(first).unwrap().mtime, 12);
    let mut copy = Vec::new();
    json::write(&model, &mut copy, true).unwrap();
    assert!(copy.windows(4).any(|window| window == b"raw\xff"));
    assert_eq!(
        json::read(Cursor::new(copy)).unwrap().directories[0].totals,
        model.directories[0].totals
    );
}
#[test]
fn unknown_nlink_shared_in_siblings_and_errors_ignore_field_order() {
    let data = b"[1,2,{},[{\"name\":\"/root\"},[{\"name\":\"a\"},{\"name\":\"h\",\"ino\":9,\"hlnkc\":true,\"asize\":7}],[{\"name\":\"b\"},{\"name\":\"h\",\"ino\":9,\"hlnkc\":true,\"asize\":7}]]]";
    let model = json::read(Cursor::new(data)).unwrap();
    assert_eq!(model.directories[0].totals.shared_apparent, 0);
    for entry in model.children(0) {
        assert_eq!(model.totals(entry).shared_apparent, 7);
    }
    for fields in [
        "\"nlink\":2,\"excluded\":\"pattern\"",
        "\"excluded\":\"pattern\",\"nlink\":2",
    ] {
        let model = json::read(Cursor::new(format!(
            "[1,2,{{}},[{{\"name\":\"/\"}},{{\"name\":\"entry\",{fields},\"dsize\":512}}]]"
        )))
        .unwrap();
        let entry = model.children(0).next().unwrap();
        assert_eq!(model.entry(entry).kind(), Kind::Excluded);
        assert_eq!(model.entry(entry).allocated(), 0);
    }
}
#[test]
fn malformed_components_numbers_and_every_truncation_are_rejected() {
    for name in ["", ".", "..", "bad/name", "bad\\u0000name"] {
        assert!(
            json::read(Cursor::new(format!(
                "[1,2,{{}},[{{\"name\":\"/\"}},{{\"name\":\"{name}\"}}]]"
            )))
            .is_err()
        );
    }
    for input in [
        b"[1,2,{},[{\"name\":\"/\",\"asize\":18446744073709551616}]]".as_slice(),
        b"[1,2,{},[{\"name\":\"/\"},{\"name\":\"\\ud800\"}]]",
        b"[1,2,{},[{\"name\":\"/\"},]]",
    ] {
        assert!(json::read(Cursor::new(input)).is_err());
    }
    let complete = fixture();
    for length in 0..complete.len() {
        assert!(json::read(Cursor::new(&complete[..length])).is_err());
    }
}
