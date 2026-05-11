//! Integration test verifying the WIT interface definition exists and
//! contains the expected interfaces and world.

#[test]
fn wit_file_exists_and_has_expected_content() {
    let wit_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/hexon-plugin.wit");
    assert!(
        wit_path.exists(),
        "WIT file should exist at {:?}",
        wit_path
    );

    let content = std::fs::read_to_string(&wit_path).expect("Should be able to read WIT file");

    // Verify package declaration
    assert!(
        content.contains("package fractalengine:plugin@1.0.0"),
        "WIT should declare package fractalengine:plugin@1.0.0"
    );

    // Verify record types
    assert!(
        content.contains("record node-snapshot"),
        "WIT should define node-snapshot record"
    );
    assert!(
        content.contains("record scene-change"),
        "WIT should define scene-change record"
    );

    // Verify interfaces
    assert!(
        content.contains("interface node-api"),
        "WIT should define node-api interface"
    );
    assert!(
        content.contains("interface plugin-log"),
        "WIT should define plugin-log interface"
    );
    assert!(
        content.contains("interface scene-hooks"),
        "WIT should define scene-hooks interface"
    );

    // Verify node-api functions
    assert!(
        content.contains("get-node:"),
        "node-api should declare get-node"
    );
    assert!(
        content.contains("set-property:"),
        "node-api should declare set-property"
    );
    assert!(
        content.contains("query-nodes:"),
        "node-api should declare query-nodes"
    );
    assert!(
        content.contains("create-node:"),
        "node-api should declare create-node"
    );
    assert!(
        content.contains("delete-node:"),
        "node-api should declare delete-node"
    );

    // Verify scene-hooks functions
    assert!(
        content.contains("on-install:"),
        "scene-hooks should declare on-install"
    );
    assert!(
        content.contains("on-activate:"),
        "scene-hooks should declare on-activate"
    );
    assert!(
        content.contains("on-deactivate:"),
        "scene-hooks should declare on-deactivate"
    );
    assert!(
        content.contains("on-uninstall:"),
        "scene-hooks should declare on-uninstall"
    );
    assert!(
        content.contains("on-scene-change:"),
        "scene-hooks should declare on-scene-change"
    );
    assert!(
        content.contains("on-tick:"),
        "scene-hooks should declare on-tick"
    );

    // Verify logging functions
    assert!(
        content.contains("log-debug:"),
        "plugin-log should declare log-debug"
    );
    assert!(
        content.contains("log-info:"),
        "plugin-log should declare log-info"
    );
    assert!(
        content.contains("log-warn:"),
        "plugin-log should declare log-warn"
    );
    assert!(
        content.contains("log-error:"),
        "plugin-log should declare log-error"
    );

    // Verify world
    assert!(
        content.contains("world fractal-plugin"),
        "WIT should define fractal-plugin world"
    );
    assert!(
        content.contains("import node-api"),
        "fractal-plugin world should import node-api"
    );
    assert!(
        content.contains("import plugin-log"),
        "fractal-plugin world should import plugin-log"
    );
    assert!(
        content.contains("export scene-hooks"),
        "fractal-plugin world should export scene-hooks"
    );
}
