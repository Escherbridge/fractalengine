# lakeside-scene

Demo `scene` hexon: 4 nodes near Lake Zurich in `entities/nodes.json`
(deserializes into `Vec<fe_format::ExportNode>`):

- Two GPX waypoints (`gpx_type: "waypoint"`) mirroring the start/finish of
  `morning-run-gpx`.
- One terrain anchor whose `hexon_ref` points at
  `hexon://did:key:z6MkSampleAlpineGis/alpine-demo-terrain@0.1.0` (also
  declared in the manifest `dependencies`), with a `hexon_installed` log op.
- One plain info node.

`schema.json` carries the field defs used by the node properties. Packed via
`HexonArchive::export_scene` (which writes `entities/nodes.json`,
`entities/field_defs.json`, and `schema.json` into the zip).

Build: `cargo run -p fe-hexon --example build_sample_hexons`
