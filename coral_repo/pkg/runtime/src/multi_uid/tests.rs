//! Unit tests for subordinate UID/GID range parsing and mapping construction.

use super::*;

#[test]
fn parses_subid_entries_by_username_or_numeric_uid() {
    let sample = "\
# comment
alice:100000:65536
1000:200000:1000
bob:300000:65536
badline
zero:0:100
empty:400000:0
";
    let ranges = parse_subid_ranges(sample, Some("alice"), 1000);
    assert_eq!(
        ranges,
        vec![
            SubIdRange {
                start: 100_000,
                count: 65_536
            },
            SubIdRange {
                start: 200_000,
                count: 1_000
            }
        ]
    );
}

#[test]
fn builds_subid_mappings_clamped_to_65535_subids() {
    let ranges = vec![SubIdRange {
        start: 100_000,
        count: 65_536,
    }];
    let maps = build_subid_mappings(1000, &ranges, 65_535);
    assert_eq!(
        maps,
        vec![
            LinuxIdMapping {
                container_id: 0,
                host_id: 1000,
                size: 1
            },
            LinuxIdMapping {
                container_id: 1,
                host_id: 100_000,
                size: 65_535
            }
        ]
    );

    let args = format_helper_args(4242, &maps);
    assert_eq!(
        args,
        vec!["4242", "0", "1000", "1", "1", "100000", "65535"]
    );
}

#[test]
fn skips_subid_range_overlapping_host_id() {
    let ranges = vec![
        SubIdRange {
            start: 500,
            count: 1000,
        },
        SubIdRange {
            start: 100_000,
            count: 2000,
        },
    ];
    let maps = build_subid_mappings(1000, &ranges, 65_535);
    assert_eq!(maps.len(), 2);
    assert_eq!(maps[1].host_id, 100_000);
    assert_eq!(maps[1].size, 2000);
}
