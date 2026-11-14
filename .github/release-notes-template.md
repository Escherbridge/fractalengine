## Downloads

| Download | Platform | What |
|----------|----------|------|
| `fractalengine-windows-x86_64.zip` | Windows x86_64 | Desktop app |
| `fractalengine-windows-aarch64.zip` | Windows ARM64 | Desktop app |
| `fractalengine-linux-x86_64.tar.gz` | Linux x86_64 (glibc 2.39+) | Desktop app |
| `fe-relay-windows-x86_64.zip` | Windows x86_64 | Headless relay |
| `fe-relay-linux-x86_64.tar.gz` | Linux x86_64 (glibc 2.39+) | Headless relay |
| `fe-relay-linux-musl-x86_64.tar.gz` | Linux x86_64 (static musl) | Headless relay |
| `fe-relay-linux-musl-aarch64.tar.gz` | Linux ARM64 (static musl) | Headless relay |
| `SHA256SUMS` | — | Checksums |
| `SHA256SUMS.minisig` | — | Signature over `SHA256SUMS` (when checksums are signed) |

Extract the whole archive and read `QUICKSTART.txt` inside it. Keep the
`assets` folder next to the desktop app: the app keeps its `data` folder there.
Verify downloads as described in the Verify section below.

Windows binaries signed: {{WINDOWS_SIGNED}}

Checksums signed: {{CHECKSUMS_SIGNED}}

Docker: `docker pull ghcr.io/{{OWNER}}/fractalengine-relay:{{TAG}}`

## Verify

1. Checksum signature first (when checksums are signed). Take the public key from the repository,
   [`docs/release-signing.pub` at {{TAG}}](https://github.com/{{REPOSITORY}}/blob/{{TAG}}/docs/release-signing.pub),
   not from this release page, save it as `release-signing.pub`, then:

   ```sh
   minisign -V -p release-signing.pub -m SHA256SUMS
   ```

   It must print `Signature and comment signature verified` and the trusted comment
   `fractalengine {{TAG}} SHA256SUMS`; a different tag means the file belongs to another release.
   A valid signature over `SHA256SUMS` covers every archive listed in it, Linux `.tar.gz`
   files included. Details: [docs/release-signing.md](https://github.com/{{REPOSITORY}}/blob/{{TAG}}/docs/release-signing.md).

2. Checksums (every archive, any platform), trusted once step 1 passes:

   ```sh
   sha256sum -c SHA256SUMS --ignore-missing
   ```

3. Windows Authenticode (after extracting the zip):

   ```powershell
   Get-AuthenticodeSignature .\fractalengine.exe
   ```

   It must report `Status : Valid` and the expected publisher.

## Windows SmartScreen

Early releases may still show a SmartScreen warning even when signed: reputation builds up
per publisher over several releases. Choosing **More info**, then **Run anyway** is expected
and is not a sign of tampering; the checks above are how to confirm a download is genuine.
