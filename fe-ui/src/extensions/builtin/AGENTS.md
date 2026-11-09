# fe-ui/src/extensions/builtin

## §builtin (extension_platform_20261107 P5.5, R33)

`copy_for_bi.rs` turns the Copy-for-BI card rows into `ConnectorDescriptor`s (id, label, pure builder). The builders
stay compiled in `gis/egress_strings.rs`: no package list can revoke or reorder them, and `snippet_rows` always renders them
first, so the card strings are byte-identical to the pre-descriptor card. `BoundSnippet` is the reply shape of an
`egress.snippet` binding (text only, <= 16 KiB, shown for copying, never run); empty or oversized ones are skipped.

`panels/egress_card.rs` renders the card by looping `snippet_rows` into `copy_row` (G5), so the labels, and with them
the accessible names, are the descriptor labels; it passes no bound snippets until BND-10c. `extensions/mod.rs` declares
`builtin` (G6). Test filter to register in conductor/test-batches.md: `extensions::builtin::`.
