# Release signing (Windows)

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
removes the "unknown publisher" warning; reputation with SmartScreen builds over time.

## Verifying a download

- Checksum: `sha256sum -c SHA256SUMS --ignore-missing` (or `Get-FileHash -Algorithm SHA256 <file>`
  and compare with the `SHA256SUMS` line).
- Signature (after extracting the zip): `Get-AuthenticodeSignature .\fractalengine.exe`
  must report `Status : Valid` and the expected publisher.
