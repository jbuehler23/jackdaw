//! The editor a test builds keeps away from the config of whoever runs it.

use crate::util;

#[test]
fn an_editor_test_app_reads_and_writes_a_scratch_config_directory() {
    let _app = util::editor_test_app();
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    for path in [
        jackdaw_env::paths::config_dir(),
        jackdaw_env::paths::keymap_path(),
        jackdaw_env::paths::keybinds_path(),
        jackdaw_env::paths::recent_file_path(),
        jackdaw_env::paths::state_dir(),
    ] {
        let path = path.expect("a config path");
        assert!(
            path.starts_with(scratch),
            "a test reaches {}, outside {}",
            path.display(),
            scratch.display()
        );
    }
}
