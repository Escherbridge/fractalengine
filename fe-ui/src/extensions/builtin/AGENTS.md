# fe-ui/src/extensions/builtin

## §builtin (extension_platform_20261107 P5.5, R33)

`copy_for_bi.rs` turns the Copy-for-BI card rows into `ConnectorDescriptor`s (id, label, pure builder). The builders
stay compiled in `gis/egress_strings.rs`: no package list can revoke or reorder them, and `snippet_rows` always renders them
first, so the card strings are byte-identical to the pre-descriptor card. `BoundSnippet` is the reply shape of an
`egress.snippet` binding (text only, <= 16 KiB, shown for copying, never run); empty or oversized ones are skipped.

Wiring owed (files this slice does not own): `pub mod builtin;` in `extensions/mod.rs`, and `panels/egress_card.rs` looping
`snippet_rows` into `copy_row` instead of its five literal calls. Test filter to register in conductor/test-batches.md:
`extensions::builtin::`.
