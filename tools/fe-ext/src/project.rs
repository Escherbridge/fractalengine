//! Project layout: `extension.json` at the root, the built component at `dist/component.wasm`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// An extension project directory.
pub struct Project {
    pub dir: PathBuf,
}

impl Project {
    pub fn open(dir: &Path) -> Result<Self> {
        if !dir.join("extension.json").is_file() {
            anyhow::bail!("{} has no extension.json; run fe-ext from a project directory or pass --project", dir.display());
        }
        Ok(Self { dir: dir.to_path_buf() })
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join("extension.json")
    }

    pub fn read_manifest(&self) -> Result<Vec<u8>> {
        std::fs::read(self.manifest_path()).with_context(|| format!("reading {}", self.manifest_path().display()))
    }

    /// Where `build` leaves the component and `pack` / `run` read it.
    pub fn component_path(&self) -> PathBuf {
        self.dir.join("dist").join("component.wasm")
    }

    pub fn read_component(&self, override_path: Option<&Path>) -> Result<Vec<u8>> {
        let path = override_path.map_or_else(|| self.component_path(), Path::to_path_buf);
        std::fs::read(&path).with_context(|| format!("reading component {} (run `fe-ext build` first)", path.display()))
    }

    /// The Cargo package name, which names the compiled artifact.
    pub fn crate_name(&self) -> Result<String> {
        let text = std::fs::read_to_string(self.dir.join("Cargo.toml")).context("reading Cargo.toml")?;
        let doc: toml::Value = toml::from_str(&text).context("parsing Cargo.toml")?;
        doc.get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Cargo.toml has no [package] name"))
    }
}
