# packaging/ — release archive layout

`scripts/package_release.py` turns a built binary into the archive users
download. Both workflows call it: `build-artifacts.yml` for the rolling
`latest` prerelease, and `release.yml` for `v*` tags. The QUICKSTART templates
in this directory hold the user-facing text. The script fills in the
`@@NAME@@` placeholders for each platform and exits with an error if a
placeholder is left unfilled.

## Archives

| Archive | Contents |
|---|---|
| `fractalengine-<platform>.zip` (Windows) / `.tar.gz` (Linux) | `fractalengine[.exe]`, `assets/samples/`, `QUICKSTART.txt`, `LICENSE-APACHE`, `THIRD-PARTY-LICENSES.md` |
| `fe-relay-<platform>.zip` / `.tar.gz` | `fe-relay[.exe]`, `QUICKSTART.txt`, `LICENSE-APACHE`, `THIRD-PARTY-LICENSES.md` |

Each archive unpacks into one folder with the same name as the archive. In a
`.tar.gz`, the binary is mode 755 and every other file is 644, with owner 0:0.
The binary's executable bit therefore survives `actions/upload-artifact`, which
drops Unix permissions on loose files.

## Why `assets/` sits next to the binary

The GUI's only file-source assets are the sample models
(`fractalengine/assets/samples/` in the repo). Imported models live in the
content-addressed blob store, not in `assets/`. The binary detects the
packaged layout from `<exe dir>/assets`. When it finds it, it does two things:

- It serves assets from that folder.
- It changes its working directory to the exe folder, so that `data/` (the
  database, the P2P store and the panic log) stays beside the binary.

See fe-runtime `src/AGENTS.md` §packaged-layout. **Removing `assets/` from an
archive silently turns this behaviour off**, and the app falls back to
CWD-relative paths. If you add a runtime asset tree, add it to
`GUI_ASSET_TREES` in the script.

## Checking the packaging locally

You can run the script on a placeholder binary to inspect the layout:

```bash
echo stub > /tmp/fractalengine
python3 scripts/package_release.py --kind gui --platform linux-x86_64 \
  --binary /tmp/fractalengine --out-dir /tmp/dist
tar -tvzf /tmp/dist/fractalengine-linux-x86_64.tar.gz
```
