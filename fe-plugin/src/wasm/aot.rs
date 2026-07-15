//! Ahead-of-time (AOT) compilation cache for Wasm plugins.
//!
//! Stores precompiled `.cwasm` files keyed by the BLAKE3 hash of the
//! source WebAssembly bytes. This avoids recompiling the same module
//! across restarts and speeds up plugin loading.
//!
//! # File naming
//!
//! ```text
//! {cache_dir}/{blake3_hex}.cwasm
//! ```
//!
//! The BLAKE3 hash is computed over the raw `.wasm` bytes and
//! hex-encoded (64 chars) for the filename.

use std::path::{Path, PathBuf};

use blake3::Hasher;

/// AOT compilation cache backed by the filesystem.
///
/// Stores `.cwasm` files in a directory, keyed by BLAKE3 hash of the
/// source Wasm bytes. On load, checks the cache first before falling
/// back to JIT compilation.
#[derive(Debug, Clone)]
pub struct AotCache {
    /// Root directory where `.cwasm` files are stored.
    cache_dir: PathBuf,
}

impl AotCache {
    /// Create a new AOT cache rooted at the given directory.
    ///
    /// The directory is created if it doesn't exist.
    pub fn new(cache_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let cache_dir = cache_dir.into();
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Compute the BLAKE3 hash of Wasm bytes and return the hex string.
    pub fn hash_bytes(wasm_bytes: &[u8]) -> String {
        let mut hasher = Hasher::new();
        hasher.update(wasm_bytes);
        hasher.finalize().to_hex().to_string()
    }

    /// Get the cache file path for a given hash.
    fn cache_path(&self, hash: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.cwasm", hash))
    }

    /// Look up a precompiled module in the cache.
    ///
    /// Returns the `.cwasm` bytes if a matching file exists, or `None`.
    pub fn lookup(&self, wasm_bytes: &[u8]) -> Option<Vec<u8>> {
        let hash = Self::hash_bytes(wasm_bytes);
        let path = self.cache_path(&hash);

        if path.exists() {
            match std::fs::read(&path) {
                Ok(data) => {
                    tracing::debug!(hash, path = ?path, "AOT cache hit");
                    Some(data)
                }
                Err(e) => {
                    tracing::warn!(hash, path = ?path, error = %e, "AOT cache read failed");
                    None
                }
            }
        } else {
            None
        }
    }

    /// Get or compile: check the cache first, and if not found, compile
    /// the module using the provided engine and store the result.
    ///
    /// Returns the compiled `.cwasm` bytes (either from cache or freshly compiled).
    pub fn get_or_compile(
        &self,
        engine: &wasmtime::Engine,
        wasm_bytes: &[u8],
    ) -> Result<Vec<u8>, crate::wasm::WasmError> {
        let hash = Self::hash_bytes(wasm_bytes);
        let path = self.cache_path(&hash);

        // Check cache first
        if path.exists() {
            let data = std::fs::read(&path).map_err(|e| {
                crate::wasm::WasmError::AotCacheError(format!(
                    "Failed to read cached file {:?}: {}",
                    path, e
                ))
            })?;
            tracing::debug!(hash, "AOT cache hit");
            return Ok(data);
        }

        // Compile and cache
        tracing::debug!(hash, "AOT cache miss, compiling");
        let module = wasmtime::Module::new(engine, wasm_bytes)?;
        let compiled = module
            .serialize()
            .map_err(|e| crate::wasm::WasmError::PrecompileError(e.to_string()))?;

        // Write to cache
        std::fs::write(&path, &compiled).map_err(|e| {
            crate::wasm::WasmError::AotCacheError(format!(
                "Failed to write cache file {:?}: {}",
                path, e
            ))
        })?;

        tracing::info!(hash, path = ?path, bytes = compiled.len(), "AOT compiled and cached");
        Ok(compiled)
    }

    /// Load a deserialized module from precompiled bytes.
    ///
    /// Uses [`wasmtime::Module::deserialize`] for fast loading without
    /// recompilation.
    pub fn load_compiled(
        &self,
        engine: &wasmtime::Engine,
        compiled_bytes: &[u8],
    ) -> Result<wasmtime::Module, crate::wasm::WasmError> {
        // SAFETY: deserialize is safe as long as the bytes were produced
        // by serialize on a compatible engine (same wasmtime version, same CPU).
        let module = unsafe { wasmtime::Module::deserialize(engine, compiled_bytes) }?;
        Ok(module)
    }

    /// Remove a cached entry by its source Wasm bytes hash.
    ///
    /// Returns `Ok(true)` if a file was removed, `Ok(false)` if not found.
    pub fn evict(&self, wasm_bytes: &[u8]) -> std::io::Result<bool> {
        let hash = Self::hash_bytes(wasm_bytes);
        let path = self.cache_path(&hash);

        if path.exists() {
            std::fs::remove_file(&path)?;
            tracing::debug!(hash, "AOT cache entry evicted");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all cached entries (hash strings).
    pub fn list_entries(&self) -> std::io::Result<Vec<String>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("cwasm") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    entries.push(stem.to_string());
                }
            }
        }
        Ok(entries)
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cache() -> (AotCache, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let cache = AotCache::new(tmp.path().join("aot_cache")).unwrap();
        (cache, tmp)
    }

    #[test]
    fn hash_is_deterministic() {
        let bytes = b"test wasm bytes";
        let h1 = AotCache::hash_bytes(bytes);
        let h2 = AotCache::hash_bytes(bytes);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // BLAKE3 hex = 64 chars
    }

    #[test]
    fn hash_differs_for_different_bytes() {
        let h1 = AotCache::hash_bytes(b"wasm A");
        let h2 = AotCache::hash_bytes(b"wasm B");
        assert_ne!(h1, h2);
    }

    #[test]
    fn get_or_compile_creates_cache() {
        let (cache, _tmp) = test_cache();
        let engine = wasmtime::Engine::default();

        let wat = r#"(module (func (export "hello")))"#;
        let bytes = wat::parse_str(wat).unwrap();

        let compiled = cache.get_or_compile(&engine, &bytes);
        assert!(compiled.is_ok());
        assert!(!compiled.as_ref().unwrap().is_empty());

        // Check the cache file exists
        let hash = AotCache::hash_bytes(&bytes);
        let cache_path = cache.cache_path(&hash);
        assert!(cache_path.exists());
    }

    #[test]
    fn get_or_compile_cache_hit() {
        let (cache, _tmp) = test_cache();
        let engine = wasmtime::Engine::default();

        let wat = r#"(module (func (export "hello")))"#;
        let bytes = wat::parse_str(wat).unwrap();

        // First compile
        let first = cache.get_or_compile(&engine, &bytes).unwrap();

        // Second lookup should hit cache
        let second = cache.get_or_compile(&engine, &bytes).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn lookup_miss_returns_none() {
        let (cache, _tmp) = test_cache();
        let result = cache.lookup(b"nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn evict_removes_cache_entry() {
        let (cache, _tmp) = test_cache();
        let engine = wasmtime::Engine::default();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();

        cache.get_or_compile(&engine, &bytes).unwrap();
        assert!(cache.lookup(&bytes).is_some());

        let removed = cache.evict(&bytes).unwrap();
        assert!(removed);
        assert!(cache.lookup(&bytes).is_none());
    }

    #[test]
    fn evict_nonexistent_returns_false() {
        let (cache, _tmp) = test_cache();
        let removed = cache.evict(b"ghost").unwrap();
        assert!(!removed);
    }

    #[test]
    fn list_entries_returns_cached_hashes() {
        let (cache, _tmp) = test_cache();
        let engine = wasmtime::Engine::default();

        let wat1 = r#"(module)"#;
        let wat2 = r#"(module (func))"#;
        let bytes1 = wat::parse_str(wat1).unwrap();
        let bytes2 = wat::parse_str(wat2).unwrap();

        cache.get_or_compile(&engine, &bytes1).unwrap();
        cache.get_or_compile(&engine, &bytes2).unwrap();

        let entries = cache.list_entries().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn load_compiled_deserializes() {
        let (cache, _tmp) = test_cache();
        let engine = wasmtime::Engine::default();

        let wat = r#"(module (func (export "test")))"#;
        let bytes = wat::parse_str(wat).unwrap();

        let compiled = cache.get_or_compile(&engine, &bytes).unwrap();
        let module = cache.load_compiled(&engine, &compiled);
        assert!(module.is_ok());
    }
}
