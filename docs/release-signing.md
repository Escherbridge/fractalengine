# Release signing

Windows binaries carry Authenticode signatures; every archive is covered by a minisign signature
over `SHA256SUMS` (see "Linux and checksum signing").

## What is wired

`build-artifacts.yml` (job `windows`) and `release.yml` (`windows-x86_64`, `windows-arm64`)
each run, after the build and before packaging:

1. **Sign** with `Azure/artifact-signing-action` (SHA256 digest, RFC3161 timestamp from
   `http://timestamp.acs.microsoft.com`) over the `.exe` files in the release target folder
   (`fractalengine.exe`, and `fe-relay.exe` where built).
2. **Verify** with `Get-AuthenticodeSignature`; any status other than `Valid` fails the job.

Both steps run only when signing is configured (`SIGNING_CONFIGURED`, derived from the
`AZURE_TENANT_ID` secret; never on pull requests in the rolling workflow). Otherwise the job
emits a `::notice::` that the binaries are unsigned and continues, so CI stays green with no
secrets. Signing precedes packaging, so `SHA256SUMS` cover the signed files. The `latest`
release notes state whether the Windows binaries are signed.

## Azure Trusted Signing setup

Repository **secrets**: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`
(a service principal with the "Trusted Signing Certificate Profile Signer" role).

Repository **variables**: `AZURE_SIGNING_ENDPOINT` (regional endpoint, e.g.
`https://eus.codesigning.azure.net`), `AZURE_SIGNING_ACCOUNT` (signing account name),
`AZURE_SIGNING_PROFILE` (certificate profile name).

## Swapping providers

Replace only the "Sign Windows binaries" step in each of the three jobs; keep its
`if: env.SIGNING_CONFIGURED == 'true'`, the verify step and the flag.

- **SignPath Foundation (free for OSS)**: upload the exes with `signpath/github-action-submit-signing-request`,
  download the signed files back over `target/release`. Change the flag to test a SignPath secret.
- **OV/EV certificate + cloud HSM**: run `signtool sign /fd SHA256 /tr <rfc3161-url> /td SHA256 ...`
  with the HSM vendor's CSP/KSP (or a tool such as AzureSignTool) over the same files.

## What users see while unsigned

Windows SmartScreen warns on first run: choose **More info**, then **Run anyway**. Signing
removes the "unknown publisher" warning; reputation with SmartScreen builds over time, per
publisher, so early signed releases may still warn. That is expected and is not tampering; the
release notes (`.github/release-notes-template.md`) say the same.

## Verifying a download

- Checksum signature (first): fetch `docs/release-signing.pub` from the repository at the release
  tag (not from the release page, which anyone able to replace assets could also replace), save it
  as `release-signing.pub` next to `SHA256SUMS` and `SHA256SUMS.minisig`, then run
  `minisign -V -p release-signing.pub -m SHA256SUMS`. A valid signature makes every line of
  `SHA256SUMS` trustworthy.
- Checksum: `sha256sum -c SHA256SUMS --ignore-missing` (or `Get-FileHash -Algorithm SHA256 <file>`
  and compare with the `SHA256SUMS` line).
- Signature (after extracting the zip): `Get-AuthenticodeSignature .\fractalengine.exe`
  must report `Status : Valid` and the expected publisher.

## Linux and checksum signing

`release.yml` (`publish` job) signs `SHA256SUMS` with minisign right after generating it and
uploads `SHA256SUMS.minisig` with the release. The Linux `.tar.gz` files are not signed one by one:
each archive's hash is a line of `SHA256SUMS`, so the one signature covers every archive on every
platform (plan Task 3.2 allows signing "over `SHA256SUMS`"). The trusted comment names the tag
(`fractalengine <tag> SHA256SUMS`) and is printed by `minisign -V`, so a signature replayed onto
another release shows the wrong tag.

How the job handles the key:

- `MINISIGN_CONFIGURED` is derived from the `MINISIGN_SECRET_KEY` secret on the job (secrets cannot
  be read in `if:`). Unset: the job emits `::notice::SHA256SUMS is unsigned` and publishes anyway, so
  unsigned releases and forks stay green; the release notes then say "Checksums signed: no". Once
  `docs/release-signing.pub` is committed, a missing secret is a lost or expired key, so the job
  raises a `::warning::` instead (still publishes; fix the secret and re-run before announcing).
- The key is written to `$RUNNER_TEMP/minisign.key` with mode 600, outside `release-assets/` (every
  file there is uploaded), and never printed; an `if: always()` step deletes it.
- When `docs/release-signing.pub` is committed, a verify step checks the new signature against it
  before publishing, which catches a secret that does not match the published public key.
- minisign was chosen over cosign/sigstore for a one-command check on the user's side. Both can
  coexist: adding a cosign signature later does not require removing minisign.

### Generating the key pair (manual, once)

The public key is not in the repository until a maintainer generates the pair:

```sh
minisign -G -W -p docs/release-signing.pub -s minisign.key
```

`-W` creates a passwordless key, so CI needs no password secret. An encrypted key also works when
its password is stored as the `MINISIGN_PASSWORD` secret (fed to minisign on stdin).

1. Commit `docs/release-signing.pub`.
2. Store the full contents of `minisign.key` (both lines) as the repository secret
   `MINISIGN_SECRET_KEY`.
3. Delete `minisign.key` from disk, or move it to offline storage.

### Rotating the key

Generate a new pair with the same command and replace `docs/release-signing.pub` and the
`MINISIGN_SECRET_KEY` secret together; say in the next release notes that the key changed. Older
releases keep verifying against the public key committed at their own tag.

## Release notes

The `publish` job renders `.github/release-notes-template.md` with `sed` into
`$RUNNER_TEMP/release-notes.md` and publishes it via `body_path`. Placeholders: `{{TAG}}`,
`{{OWNER}}`, `{{REPOSITORY}}`, `{{WINDOWS_SIGNED}}`, `{{CHECKSUMS_SIGNED}}`. Values reach the script
through step `env` (never interpolated into shell text) and are escaped for `sed`; a placeholder left
unrendered fails the job. Edit the template, not the workflow, to change the notes.
The `publish` and GHCR image jobs run only for tag pushes, never for `workflow_dispatch`, even when it is started from a tag.

## Hardening notes

- The signing action is pinned to a full commit SHA (v2.0.0); bump it deliberately.
- Prefer OIDC workload identity federation (azure/login with `id-token: write`, subject scoped to `main` and release tags) over a long-lived `AZURE_CLIENT_SECRET` when the signing account is set up.
