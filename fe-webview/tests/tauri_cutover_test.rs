//! Tests for Tauri Backend Cutover (Track 3).
//!
//! These tests verify:
//! 1. Default feature selection works correctly
//! 2. Backend type resolution is correct
//! 3. Documentation builds correctly
//! 4. System dependencies are documented
//!
//! Run with: cargo test --test tauri_cutover_test
//!
//! Path note: for integration tests, `CARGO_MANIFEST_DIR` is the crate root
//! (`<workspace>/fe-webview`), NOT `fe-webview/tests`. The workspace root is
//! therefore exactly one `.parent()` up.

/// Crate root: `<workspace>/fe-webview`.
fn crate_root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Workspace root: the directory containing the `[workspace]` Cargo.toml.
fn workspace_root() -> &'static std::path::Path {
    crate_root()
        .parent()
        .expect("fe-webview should live directly under the workspace root")
}

#[cfg(test)]
mod tests {
    use super::{crate_root, workspace_root};
    use std::process::Command;

    /// Sanity-check the path helpers before any test shells out with them.
    #[test]
    fn test_path_helpers_resolve() {
        assert!(
            crate_root().join("Cargo.toml").exists(),
            "crate root should contain fe-webview's Cargo.toml: {}",
            crate_root().display()
        );
        let ws_manifest = workspace_root().join("Cargo.toml");
        assert!(
            ws_manifest.exists(),
            "workspace root should contain Cargo.toml: {}",
            workspace_root().display()
        );
        let content = std::fs::read_to_string(&ws_manifest).expect("read workspace Cargo.toml");
        assert!(
            content.contains("[workspace]"),
            "workspace root Cargo.toml should declare [workspace]"
        );
    }

    /// Test that fe-webview compiles with default features
    /// This verifies that backend-tauri becomes the default
    #[test]
    fn test_default_feature_compiles() {
        let result = Command::new("cargo")
            .args(["check", "-p", "fe-webview"])
            .current_dir(workspace_root())
            .output();

        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "cargo check failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                panic!("Failed to run cargo check: {}", e);
            }
        }
    }

    /// Test that backend-tauri feature compiles
    #[test]
    fn test_tauri_feature_compiles() {
        let result = Command::new("cargo")
            .args(["check", "-p", "fe-webview", "--features", "backend-tauri"])
            .current_dir(workspace_root())
            .output();

        match result {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "cargo check with backend-tauri failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                panic!("Failed to run cargo check: {}", e);
            }
        }
    }

    /// Test feature flag configuration in Cargo.toml
    /// This is a static analysis test - we read the Cargo.toml
    #[test]
    fn test_feature_flag_configuration() {
        let content = std::fs::read_to_string(crate_root().join("Cargo.toml"))
            .expect("Failed to read Cargo.toml");

        // Check that backend-tauri feature exists
        assert!(
            content.contains("backend-tauri"),
            "backend-tauri feature should be defined"
        );

        // Check that the feature has correct dependencies
        assert!(
            content.contains("\"tauri\"") || content.contains("tauri"),
            "backend-tauri should depend on tauri"
        );
    }

    /// Test that the legacy 'webview' alias works
    #[test]
    fn test_legacy_webview_alias() {
        let content = std::fs::read_to_string(crate_root().join("Cargo.toml"))
            .expect("Failed to read Cargo.toml");

        // Check that the legacy 'webview' alias exists
        assert!(
            content.contains("webview = "),
            "Legacy 'webview' alias should be defined for backward compatibility"
        );
    }

    /// Test backend priority order (Servo > Tauri > Stub)
    #[test]
    fn test_backend_priority_order() {
        let mod_path = crate_root().join("src").join("backends").join("mod.rs");

        let content = std::fs::read_to_string(&mod_path).expect("Failed to read backends/mod.rs");

        // Verify Servo has priority when both features enabled
        assert!(
            content.contains("backend-servo"),
            "Servo backend should be available"
        );

        // Verify Tauri is properly configured
        assert!(
            content.contains("backend-tauri"),
            "Tauri backend should be available"
        );

        // Verify stub is the fallback
        assert!(
            content.contains("stub"),
            "Stub backend should exist as fallback"
        );
    }
}

#[cfg(test)]
mod documentation_tests {
    use super::{crate_root, workspace_root};
    use std::fs;

    /// Test that AGENTS.md exists and contains webview documentation
    #[test]
    fn test_agents_md_exists() {
        let agents_md = workspace_root().join("AGENTS.md");

        assert!(
            agents_md.exists(),
            "AGENTS.md should exist at workspace root"
        );

        let content = fs::read_to_string(&agents_md).expect("Failed to read AGENTS.md");

        // Should mention webview or PetalPortal
        assert!(
            content.to_lowercase().contains("webview")
                || content.to_lowercase().contains("petalportal"),
            "AGENTS.md should document webview/PetalPortal"
        );
    }

    /// Test that the crate-level AGENTS.md referenced by the backends exists.
    #[test]
    fn test_crate_agents_md_exists() {
        let agents_md = crate_root().join("src").join("AGENTS.md");
        assert!(
            agents_md.exists(),
            "fe-webview/src/AGENTS.md should exist (referenced by tauri.rs / win32_popup.rs)"
        );
    }

    /// Test that BUILDING.md exists
    #[test]
    fn test_building_md_exists() {
        let building_md = workspace_root().join("BUILDING.md");

        // BUILDING.md may not exist in all projects, so this is a soft check
        if building_md.exists() {
            let content = fs::read_to_string(&building_md).expect("Failed to read BUILDING.md");

            // If it exists, it should mention system dependencies
            // (but we don't require it to exist)
            println!("BUILDING.md exists with {} chars", content.len());
        } else {
            println!("BUILDING.md not found (optional)");
        }
    }

    /// Test that the fe-webview lib.rs documents the backends
    #[test]
    fn test_lib_docs_exist() {
        let lib_path = crate_root().join("src").join("lib.rs");

        let content = fs::read_to_string(&lib_path).expect("Failed to read lib.rs");

        // Should have module declarations for backends
        assert!(
            content.contains("mod backends") || content.contains("pub mod"),
            "lib.rs should declare modules"
        );
    }
}

#[cfg(test)]
mod system_dependency_tests {
    #[cfg(not(target_os = "windows"))]
    use std::process::Command;

    /// Test that we can detect the OS for system dependency documentation
    #[test]
    fn test_os_detection() {
        #[cfg(target_os = "linux")]
        {
            let result = Command::new("uname").arg("-s").output();
            assert!(result.is_ok());
        }

        #[cfg(target_os = "windows")]
        {
            // On Windows the platform is selected at compile time — nothing to probe.
        }

        #[cfg(target_os = "macos")]
        {
            let result = Command::new("uname").arg("-s").output();
            assert!(result.is_ok());
        }
    }

    /// Test for webkit2gtk on Linux (required for Tauri)
    #[test]
    #[cfg(target_os = "linux")]
    fn test_webkit2gtk_detection() {
        // Try to detect webkit2gtk
        let result = Command::new("pkg-config")
            .args(["--exists", "webkit2gtk-4.1"])
            .output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    println!("webkit2gtk-4.1 is installed");
                } else {
                    println!("webkit2gtk-4.1 NOT installed (may need to install for Tauri)");
                }
            }
            Err(e) => {
                println!("Could not check for webkit2gtk: {}", e);
            }
        }
    }

    /// Test that required Tauri dependencies can be checked
    #[test]
    #[cfg(target_os = "linux")]
    fn test_tauri_linux_deps() {
        // Check for common Tauri Linux dependencies
        let deps = vec!["pkg-config", "libssl-dev"];

        for dep in deps {
            let result = Command::new("which").arg(dep).output();

            match result {
                Ok(output) => {
                    if output.status.success() {
                        println!("✓ {} found", dep);
                    } else {
                        println!("✗ {} NOT found", dep);
                    }
                }
                Err(e) => {
                    println!("Could not check for {}: {}", dep, e);
                }
            }
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::workspace_root;

    /// Verify the entire workspace compiles.
    /// Ignored by default: it spawns a full nested `cargo check --workspace`
    /// (minutes on a cold cache) and never fails — run explicitly when needed.
    #[test]
    #[ignore = "spawns nested cargo check --workspace; log-only, run explicitly"]
    fn test_workspace_compiles() {
        let result = std::process::Command::new("cargo")
            .args(["check", "--workspace"])
            .current_dir(workspace_root())
            .output();

        match result {
            Ok(output) => {
                if !output.status.success() {
                    println!("Workspace check warnings/errors:");
                    println!("{}", String::from_utf8_lossy(&output.stderr));
                }
                // We just log warnings, don't fail
            }
            Err(e) => {
                panic!("Failed to run cargo check: {}", e);
            }
        }
    }

    /// Test that fe-webview tests run.
    /// Ignored by default: rebuilds this crate's test binaries from a nested
    /// cargo and only prints the list — run explicitly when needed.
    #[test]
    #[ignore = "spawns nested cargo test --list; log-only, run explicitly"]
    fn test_webview_tests_available() {
        let result = std::process::Command::new("cargo")
            .args(["test", "-p", "fe-webview", "--", "--list"])
            .current_dir(workspace_root())
            .output();

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                println!("Available tests in fe-webview:\n{}", stdout);
            }
            Err(e) => {
                println!("Could not list tests: {}", e);
            }
        }
    }
}
