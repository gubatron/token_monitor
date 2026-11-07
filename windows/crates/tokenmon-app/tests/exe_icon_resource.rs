//! The exe's application icon (#81): `build.rs` turns `assets/tokenmon.ico`
//! into `RT_ICON` frames and an `RT_GROUP_ICON` with id 1. The writer is
//! compiled into this test so the `.res` it produces is parsed on every host.

#[allow(dead_code, clippy::all, clippy::pedantic)]
#[path = "../build.rs"]
mod build_script;

use build_script::{RT_GROUP_ICON, RT_ICON, RT_VERSION, icon_entries, res_file, version_entry};

const ICO: &[u8] = include_bytes!("../assets/tokenmon.ico");

/// One parsed `.res` entry: `(type, name, data)`.
type Parsed = (u16, u16, Vec<u8>);

fn le32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes")) as usize
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().expect("2 bytes"))
}

/// A minimal `.res` reader: skips the empty first entry, returns the rest.
fn parse_res(res: &[u8]) -> Vec<Parsed> {
    let mut at = 32;
    let mut out = Vec::new();
    while at < res.len() {
        let (size, header) = (le32(res, at), le32(res, at + 4));
        assert_eq!(header, 32, "fixed-size headers only");
        assert_eq!(le16(res, at + 8), 0xFFFF, "integer type id");
        assert_eq!(le16(res, at + 12), 0xFFFF, "integer name id");
        let (kind, name) = (le16(res, at + 10), le16(res, at + 14));
        out.push((kind, name, res[at + header..at + header + size].to_vec()));
        at += (header + size).div_ceil(4) * 4;
    }
    assert_eq!(at, res.len(), "entries are 4-aligned and fill the file");
    out
}

/// Every image of the `.ico` becomes an `RT_ICON` with ids 1..=n, in order.
#[test]
fn test_every_ico_frame_becomes_an_rt_icon() {
    let parsed = parse_res(&res_file(&icon_entries(ICO).expect("valid ico")));
    let count = usize::from(le16(ICO, 4));
    let icons: Vec<_> = parsed.iter().filter(|(k, _, _)| *k == RT_ICON).collect();
    assert_eq!(icons.len(), count);
    assert!(count >= 5, "16/20/24/32/40 px frames at least");
    for (i, (_, name, data)) in icons.iter().enumerate() {
        assert_eq!(usize::from(*name), i + 1);
        let at = 6 + 16 * i;
        let (size, offset) = (le32(ICO, at + 8), le32(ICO, at + 12));
        assert_eq!(data.as_slice(), &ICO[offset..offset + size]);
    }
}

/// The group icon has id 1 (the lowest, so it is the exe's icon) and lists
/// each frame with its `RT_ICON` id and byte size.
#[test]
fn test_group_icon_id_1_lists_every_frame() {
    let parsed = parse_res(&res_file(&icon_entries(ICO).expect("valid ico")));
    let groups: Vec<_> = parsed
        .iter()
        .filter(|(k, _, _)| *k == RT_GROUP_ICON)
        .collect();
    assert_eq!(groups.len(), 1);
    let (_, name, group) = groups[0];
    assert_eq!(*name, 1);
    let count = usize::from(le16(group, 4));
    assert_eq!(
        (le16(group, 0), le16(group, 2)),
        (0, 1),
        "reserved, type icon"
    );
    assert_eq!(group.len(), 6 + 14 * count);
    for i in 0..count {
        let e = 6 + 14 * i;
        let id = le16(group, e + 12);
        let icon = parsed
            .iter()
            .find(|(k, n, _)| *k == RT_ICON && *n == id)
            .expect("group entry points at an RT_ICON");
        assert_eq!(le32(group, e + 8), icon.2.len());
        // Same width/height byte as the .ico directory.
        assert_eq!(group[e], ICO[6 + 16 * i]);
    }
}

/// The version resource still sits alongside the icons.
#[test]
fn test_version_resource_is_kept_next_to_the_icons() {
    let mut entries = icon_entries(ICO).expect("valid ico");
    entries.push(version_entry(vec![1, 2, 3, 4]));
    let parsed = parse_res(&res_file(&entries));
    assert!(
        parsed
            .iter()
            .any(|(k, n, d)| (*k, *n, d.as_slice()) == (RT_VERSION, 1, &[1, 2, 3, 4][..]))
    );
}

/// Malformed icon files are rejected instead of producing a broken resource.
#[test]
fn test_malformed_ico_is_rejected() {
    assert!(icon_entries(&[]).is_none());
    assert!(
        icon_entries(&[0, 0, 2, 0, 1, 0]).is_none(),
        "cursor, not icon"
    );
    let truncated = &ICO[..40];
    assert!(icon_entries(truncated).is_none());
}
